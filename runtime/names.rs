//! Utilities for dealing with canonical component names and pod names.
//!
//! These are the only two types of names that the Work runtime deals with,
//! and they have significant syntactic overlap,
//! so they share a single permissive parsing function: [`Name::parse`],
//! which cheaply parses anything that *might* be either of them.
//!
//! Both are always in *canonical* form,
//! where the domain is always a UUID and never an alias.
//!
//! Once parsed, use the conversion functions to validate a name as a particular type.
//! These are:
//! - [`ComponentName`]
//! - [`PodName`]
#![feature(portable_simd)]

use std::fmt::{Debug, Display, Formatter, Result as FmtResult, Write};
use std::simd::cmp::SimdPartialOrd;
use std::simd::{simd_swizzle, u8x16, u8x32};
use std::slice::from_ref;
use std::str;

use anyhow::{anyhow, Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

const DOMAIN_SEPARATOR: char = ':';
const VERSION_SEPARATOR: char = '@';
const POD_ID_SEPARATOR: char = '#';

// SIMD constants used for parsing / unparsing domain UUIDs:
const SIXTEEN_16: u8x16 = u8x16::splat(16);
const NINE_32: u8x32 = u8x32::splat(9);
const ASCII_A_32: u8x32 = u8x32::splat(b'a');
const ASCII_F_32: u8x32 = u8x32::splat(b'f');
const ASCII_ZERO_32: u8x32 = u8x32::splat(b'0');
const ASCII_NINE_32: u8x32 = u8x32::splat(b'9');
const ASCII_A_FROM_TEN_32: u8x32 = u8x32::splat(b'a' - 10);

const HEX_CHARS: &[u8] = b"0123456789abcdef";

pub type PodId = usize;

/// Permissively [parses](Self::parse) a server / version name:
///     [ domain-id ':' ] server-id [ '@' version [ '#' pod-id ] ]
///
/// Use the conversion functions to validate and convert to particular name types.
pub struct Name<'a> {
    pub domain: Option<&'a str>,
    pub server: &'a str,
    pub version: Option<&'a str>,
    pub pod: Option<&'a str>,
}

impl<'a> Name<'a> {
    /// Permissively parse a server / version name:
    ///     [ domain-id ':' ] server-id [ '@' version [ '#' pod-id ] ]
    pub fn parse(full: &'a str) -> Self {
        let mut domain: Option<&'a str> = None;
        let mut version: Option<&'a str> = None;
        let mut pod: Option<&'a str> = None;

        // Used to mark the boundaries of the server name part:
        let mut start = 0;
        let mut end = full.len();

        // Look for a domain or version separator.
        // A pod ID separator can only be present if there is a version.
        if let Some(index) = full.find(&[DOMAIN_SEPARATOR, VERSION_SEPARATOR]) {
            if full[index..].starts_with(DOMAIN_SEPARATOR) {
                // There's an explicit domain.
                domain = Some(&full[..index]);
                start = index + 1;
                if let Some(v_index) = full[start..].find(VERSION_SEPARATOR) {
                    // And a version.
                    end = start + v_index;
                    (version, pod) = parse_version(&full[end + 1..]);
                }
            } else {
                // No explicit domain, but there is a version.
                end = index;
                (version, pod) = parse_version(&full[end + 1..]);
            }
        }
        let server = &full[start..end];

        Name {
            domain,
            server,
            version,
            pod,
        }
    }

    /// If the domain and version are both explicitly present and valid,
    /// but the pod ID is absent,
    /// consume self and return an equivalent [`ComponentName`].
    pub fn component(self) -> Result<ComponentName> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                if self.pod.is_none() {
                    return ComponentName::new(DomainUuid::parse(domain)?, self.server, version);
                }
            }
        }
        Err(anyhow!("Invalid component name: {:?}", self))
    }

    /// If the domain, version, and pod ID are all explicitly present and valid,
    /// consume self and return an equivalent [`PodName`].
    /// Pod names are always fully-qualified.
    pub fn pod(self) -> Result<PodName> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                if let Some(pod) = self.pod {
                    let component =
                        ComponentName::new(DomainUuid::parse(domain)?, self.server, version)?;
                    let pod = usize::from_str_radix(pod, 16).context("Invalid pod ID")?;
                    return Ok(PodName::new(component, pod));
                }
            }
        }
        Err(anyhow!("Invalid pod name: {:?}", self))
    }
}

// Parse the part after the '@' and return a version string (always present)
// and possibly a pod ID.
fn parse_version<'a>(version: &'a str) -> (Option<&'a str>, Option<&'a str>) {
    let mut pod: Option<&'a str> = None;
    let version = version.find(POD_ID_SEPARATOR).map_or(version, |index| {
        pod = Some(&version[index + 1..]);
        &version[..index]
    });
    (Some(version), pod)
}

impl<'a> Debug for Name<'a> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_char('"')?;
        if let Some(domain) = self.domain {
            formatter.write_str(domain)?;
            formatter.write_char(DOMAIN_SEPARATOR)?;
        }
        formatter.write_str(self.server)?;
        if let Some(version) = self.version {
            formatter.write_char(VERSION_SEPARATOR)?;
            formatter.write_str(version)?;
            if let Some(pod) = self.pod {
                formatter.write_char(POD_ID_SEPARATOR)?;
                formatter.write_str(pod)?;
            }
        }
        formatter.write_char('"')
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DomainUuid {
    /// The UUID part of a canonical domain represents 128 bits.
    /// Here they are as a little-endian SIMD vector of bytes.
    pub(crate) uuid: u8x16,
}

impl DomainUuid {
    pub fn parse(uuid: &str) -> Result<Self> {
        // The hex-encoded UUID string must be 32 bytes long
        // (1 logical nibble per hex-encoded byte).
        if uuid.len() == 32 {
            Ok(Self {
                uuid: unhexify(u8x32::from_slice(uuid.as_bytes()))?,
            })
        } else {
            Err(anyhow!("Incorrect domain UUID length: {:?}", uuid.len()))
        }
    }

    pub fn new(bytes: &[u8; 16]) -> Self {
        Self {
            uuid: u8x16::from_array(*bytes),
        }
    }
}

impl Display for DomainUuid {
    /// Format the domain UUID as a 32-character hex-encoded string.
    /// Inverse of [`DomainUuid::parse`].
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        let hex_bytes = hexify(self.uuid);
        let array = hex_bytes.as_array();
        let uuid = unsafe { str::from_utf8_unchecked(array) };
        formatter.write_str(uuid)
    }
}

/// A server name:
///     domain-id ':' server-id
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerName {
    pub domain: DomainUuid,
    pub server: String,
}

impl ServerName {
    pub fn new<S>(domain: DomainUuid, server: S) -> Result<Self>
    where
        S: Into<String>,
    {
        let server: String = server.into();
        if is_valid_server_name(&server) {
            Ok(Self { domain, server })
        } else {
            Err(anyhow!("Invalid server ID: {:?}", server))
        }
    }
}

impl Display for ServerName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.domain, formatter)?;
        formatter.write_char(DOMAIN_SEPARATOR)?;
        formatter.write_str(&self.server)
    }
}

/// A component name (versioned server):
///     domain-id ':' server-id '@' version
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentName {
    pub server: ServerName,
    pub version: String,
}

impl ComponentName {
    pub fn new<S, V>(domain: DomainUuid, server: S, version: V) -> Result<Self>
    where
        S: Into<String>,
        V: Into<String>,
    {
        let version: String = version.into();
        if is_valid_version(&version) {
            Ok(Self {
                server: ServerName::new(domain, server)?,
                version,
            })
        } else {
            Err(anyhow!("Invalid version: {:?}", version))
        }
    }
}

impl Display for ComponentName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.server, formatter)?;
        formatter.write_char(VERSION_SEPARATOR)?;
        formatter.write_str(&self.version)
    }
}

/// A pod / container name:
///     domain-id ':' server-id '@' version '#' pod-id
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PodName {
    pub component: ComponentName,
    pub pod: PodId,
}

impl PodName {
    pub fn new(component: ComponentName, pod_id: PodId) -> Self {
        Self {
            component: component,
            pod: pod_id,
        }
    }
}

impl Display for PodName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.component, formatter)?;
        formatter.write_char(POD_ID_SEPARATOR)?;
        formatter.write_fmt(format_args!("{:x}", self.pod))
    }
}

/// Maximum length (inclusive) for K8s labels values.
const K8S_LABEL_VALUE_MAX_LENGTH: usize = 63;

/// Predicate for syntactically valid server names.
/// These are stored as
/// [K8s labels](https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/#syntax-and-character-set)
/// and as [resource names](https://kubernetes.io/docs/concepts/overview/working-with-objects/names/),
/// which together dictate the following constraints:
///
/// True iff `name`:
///   - Contains no more than 63 bytes.
///   - Consists of only lowercase ASCII alphanumerics and dashes.
///   - Starts with an alphabetical character and ends with an alphanumeric.
fn is_valid_server_name(name: &str) -> bool {
    lazy_static! {
        static ref PATH_RE: Regex = Regex::new(r"^[a-z](?:[0-9a-z-]{0,61}[0-9a-z])?$").unwrap();
    }
    PATH_RE.is_match(name)
}

/// Predicate for valid version strings.
///
/// True if `version` is [SemVer](https://semver.org)-compliant.
/// In addition, because versions are stored as
/// [K8s labels](https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/#syntax-and-character-set),
/// their max length is 63,
/// and they cannot contain a "build" component
/// (because they cannot contain `+`).
/// Also, the pre-release part (if any) must not end with a dash.
fn is_valid_version(version: &str) -> bool {
    lazy_static! {
        static ref VERSION_RE: Regex = Regex::new(concat!(
            // Core part consists of "numeric identifiers",
            // which must be positive integers or `0`.
            r"^(?:0|[1-9][0-9]*)",   // major
            r"\.(?:0|[1-9][0-9]*)",  // minor
            r"\.(?:0|[1-9][0-9]*)",  // patch
            // Optional pre-release consists of `-`
            // followed by either a numeric identifier,
            // or a sequence of alphanumerics and dashes
            // that must contain at least 1 non-digit
            // and cannot end with a dash.
            r"(?:-(?:0|[1-9][0-9]*|[0-9A-Za-z-]*(?:[A-Za-z]|[A-Za-z-][0-9A-Za-z-]*[0-9A-Za-z])))?$",
        ))
        .unwrap();
    }
    version.len() <= K8S_LABEL_VALUE_MAX_LENGTH && VERSION_RE.is_match(version)
}

#[inline(always)]
/// Convert an array of sixteen arbitrary bytes
/// to a nibblewise little-endian hex-encoded array of thirty-two bytes.
/// This is the inverse of [unhexify].
pub fn hexify(bytes: u8x16) -> u8x32 {
    let lower_nibbles = bytes % SIXTEEN_16;
    let upper_nibbles = bytes / SIXTEEN_16;
    let nibbles = simd_swizzle!(
        lower_nibbles,
        upper_nibbles,
        [
            0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23, 8, 24, 9, 25, 10, 26, 11, 27,
            12, 28, 13, 29, 14, 30, 15, 31,
        ],
    );
    nibbles
        + nibbles
            .simd_gt(NINE_32)
            .select(ASCII_A_FROM_TEN_32, ASCII_ZERO_32)
}

#[inline(always)]
/// Decode a nibblewise little-endian hex-encoded array of thirty-two bytes.
/// This is the inverse of [hexify].
pub fn unhexify(hex_bytes: u8x32) -> Result<u8x16> {
    // Check that nothing is outside the range `[0-f]` or inside `[9-a]`.
    if hex_bytes.simd_lt(ASCII_ZERO_32).any()
        || hex_bytes.simd_gt(ASCII_F_32).any()
        || (hex_bytes.simd_gt(ASCII_NINE_32) & hex_bytes.simd_lt(ASCII_A_32)).any()
    {
        return Err(anyhow!(
            "Invalid hex characters: {:?}",
            String::from_utf8_lossy(hex_bytes.as_array())
        ));
    }
    // Convert to logical nibbles
    // by subtracting ASCII 'a' from each byte that's greater than ASCII '9',
    // and subtracting ASCII '0' from all other bytes.
    let nibbles = hex_bytes
        - hex_bytes
            .simd_gt(ASCII_NINE_32)
            .select(ASCII_A_FROM_TEN_32, ASCII_ZERO_32);
    // Deinterleave the nibbles of each logical byte.
    let lower_nibbles = simd_swizzle!(
        nibbles,
        [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30]
    );
    let upper_nibbles = simd_swizzle!(
        nibbles,
        [1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31]
    );
    // Recombine the nibbles of each logical byte.
    Ok(lower_nibbles + (upper_nibbles * SIXTEEN_16))
}

/// Hex-encode a string,
/// returning a new string with equivalient data and double the length,
/// using only the characters `[0-9a-f]`,
/// nibblewise little-endian (lower nibble comes first).
/// This is the inverse of [unhexify_string].
pub fn hexify_string(string: &str) -> String {
    let mut output = Vec::with_capacity(string.len() * 2);
    let chunks = string.as_bytes().chunks_exact(16);
    let remainder = chunks.remainder();

    for chunk in chunks {
        output.extend_from_slice(hexify(u8x16::from_slice(chunk)).as_array());
    }
    for byte in remainder {
        output.push(HEX_CHARS[(byte & 0xf) as usize]);
        output.push(HEX_CHARS[(byte >> 4) as usize]);
    }

    unsafe { String::from_utf8_unchecked(output) }
}

/// This is the inverse of [hexify_string].
pub fn unhexify_string(hex_string: &str) -> Result<String> {
    let hex_length = hex_string.len();
    if hex_length % 2 != 0 {
        return Err(anyhow!("Odd-length hex string"));
    }
    let mut output = Vec::with_capacity(hex_length / 2);
    let chunks = hex_string.as_bytes().chunks_exact(32);
    let remainder = chunks.remainder().chunks_exact(2);

    for chunk in chunks {
        output.extend_from_slice(unhexify(u8x32::from_slice(chunk))?.as_array());
    }
    for byte in remainder {
        output.push(unhexify_nibble(byte[0])? + (unhexify_nibble(byte[1])? << 4));
    }

    Ok(String::from_utf8(output).context("Hex string represents invalid UTF-8")?)
}

#[inline(always)]
fn unhexify_nibble(hex_nibble: u8) -> Result<u8> {
    if hex_nibble >= b'0' && hex_nibble <= b'9' {
        Ok(hex_nibble - b'0')
    } else if hex_nibble >= b'a' && hex_nibble <= b'f' {
        Ok(hex_nibble - (b'a' - 10))
    } else {
        Err(anyhow!(
            "Invalid hex character: {:?}",
            String::from_utf8_lossy(from_ref(&hex_nibble))
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Domains are parsed with little-endian order
    // so each byte looks inverted when written in typical Arabic notation.
    const GOOD_DOMAIN_BYTES: [u8; 16] = [
        0x21, 0x43, 0x65, 0x87, 0x09, 0xBA, 0xDC, 0xFE, 0xF0, 0xE9, 0x8D, 0x7C, 0x56, 0xB4, 0x23,
        0x1a,
    ];
    const GOOD_DOMAIN: &str = "1234567890abcdef0f9ed8c7654b32a1";
    const GOOD_SERVER: &str = "some-server-id";
    const GOOD_VERSION: &str = "10.0.456-pre-release5";
    const GOOD_POD_ID: &str = "a10f0";
    const GOOD_COMPONENT: &str =
        "1234567890abcdef0f9ed8c7654b32a1:some-server-id@10.0.456-pre-release5";
    const GOOD_POD: &str =
        "1234567890abcdef0f9ed8c7654b32a1:some-server-id@10.0.456-pre-release5#a10f0";

    #[test]
    fn parse_component() {
        let component = Name::parse(GOOD_COMPONENT).component();

        assert!(component.is_ok());
        let component = component.unwrap();
        assert_eq!(
            component.server.domain,
            DomainUuid::parse(GOOD_DOMAIN).unwrap(),
        );
        assert_eq!(component.server.server, GOOD_SERVER);
        assert_eq!(component.version, GOOD_VERSION);
        assert_eq!(format!("{component}"), GOOD_COMPONENT);
    }

    #[test]
    fn parse_pod_ids_good() {
        let good_pod_ids = vec![
            ("1a2f", 0x1a2fusize),
            ("abcdefff", 0xabcdefff),
            ("0", 0x0),
            ("ffffffffffffffff", 0xffffffffffffffff),
        ];

        for (pod_id_str, pod_id) in good_pod_ids.iter() {
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVER}@{GOOD_VERSION}#{pod_id_str}");

            let pod_name = Name::parse(&name).pod();

            assert!(pod_name.is_ok());
            let pod_name = pod_name.unwrap();
            assert_eq!(
                pod_name.component.server.domain,
                DomainUuid::parse(GOOD_DOMAIN).unwrap(),
            );
            assert_eq!(pod_name.component.server.server, GOOD_SERVER);
            assert_eq!(pod_name.component.version, GOOD_VERSION);
            assert_eq!(&pod_name.pod, pod_id);
            assert_eq!(format!("{pod_name}"), name);
        }
    }

    #[test]
    fn parse_pod_ids_bad() {
        let bad_pod_ids = vec!["abcdefg", "-1", "10000000000000000"];

        for pod_id in bad_pod_ids.iter() {
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVER}@{GOOD_VERSION}#{pod_id}");

            let pod_name = Name::parse(&name).pod();

            assert!(pod_name.is_err());
            assert_eq!(pod_name.unwrap_err().to_string(), "Invalid pod ID",);
        }
    }

    #[test]
    fn parse_versions_good() {
        let good_versions = vec![
            "1.0.0",
            "1.2.3-pre-release",
            "1.2.3-0",
            "1.2.3-123",
            "1.2.3-00a",
        ];

        for version in good_versions.iter() {
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVER}@{version}");

            let component = Name::parse(&name).component();

            assert!(component.is_ok());
            let component = component.unwrap();
            assert_eq!(
                component.server.domain,
                DomainUuid::parse(GOOD_DOMAIN).unwrap(),
            );
            assert_eq!(component.server.server, GOOD_SERVER);
            assert_eq!(component.version, *version);
            assert_eq!(format!("{component}"), name);
        }
    }

    #[test]
    fn parse_versions_bad() {
        let bad_versions = vec![
            "1.0.00",            // Double zero goes against spec.
            "1.2.3+build",       // Can't handle '+'.
            "1.2.3-ends-dash-",  // Can't end with a dash.
            "1.2.3-00",          // Can't have double-zero in the pre-release either.
            // Too long:
            "1234567890.1234567890.1234567890-ABCDEFGHIJKLMNOPQRSTUVWXYZ-abcdefghijklmnopqrstuvwxyz",
        ];

        for version in bad_versions.iter() {
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVER}@{version}");
            let component = Name::parse(&name).component();

            assert!(component.is_err());
            assert_eq!(
                component.unwrap_err().to_string(),
                format!("Invalid version: {:?}", version),
            );
        }
    }

    #[test]
    fn parse_server_good() {
        let good_servers = vec!["simple", "contains-dash", "ends-with-digit-1"];

        for server in good_servers.iter() {
            let name = format!("{GOOD_DOMAIN}:{server}@{GOOD_VERSION}");

            let component = Name::parse(&name).component();

            assert!(component.is_ok());
            let component = component.unwrap();
            assert_eq!(
                component.server.domain,
                DomainUuid::parse(GOOD_DOMAIN).unwrap(),
            );
            assert_eq!(component.server.server, *server);
            assert_eq!(component.version, GOOD_VERSION);
            assert_eq!(format!("{component}"), name);
        }
    }

    #[test]
    fn parse_server_bad() {
        let bad_servers = vec![
            "",
            "contains.period",
            "contains-Capital",
            "contains_underscore",
            "-starts-with-dash",
            "1-starts-with-digit",
            "this-server-name-would-be-too-long-due-to-being-over-sixty-three",
        ];

        for server in bad_servers.iter() {
            let name = format!("{GOOD_DOMAIN}:{server}@{GOOD_VERSION}");

            let component = Name::parse(&name).component();

            assert!(
                component.is_err(),
                "Expected server ID {:?} to be invalid",
                server
            );
            assert_eq!(
                component.unwrap_err().to_string(),
                format!("Invalid server ID: {:?}", server),
            );
        }
    }

    #[test]
    fn parse_domain_success() {
        let domain_uuid = DomainUuid::parse(GOOD_DOMAIN);

        assert!(domain_uuid.is_ok());
        assert_eq!(domain_uuid.unwrap().uuid.as_array(), &GOOD_DOMAIN_BYTES);
    }

    #[test]
    fn format_domain_success() {
        let domain_uuid = DomainUuid {
            uuid: GOOD_DOMAIN_BYTES.into(),
        };

        let domain = format!("{domain_uuid}");

        assert_eq!(domain, GOOD_DOMAIN);
    }

    #[test]
    fn parse_domain_short() {
        // 31 characters instead of 32.
        let domain = "1234567890abcdef1234567890abcde";

        let domain_uuid = DomainUuid::parse(domain);

        assert!(domain_uuid.is_err());
        assert_eq!(
            domain_uuid.unwrap_err().to_string(),
            "Incorrect domain UUID length: 31",
        );
    }

    #[test]
    fn parse_domain_long() {
        // 33 characters instead of 32.
        let domain = "1234567890abcdef1234567890abcdef1";

        let domain_uuid = DomainUuid::parse(domain);

        assert!(domain_uuid.is_err());
        assert_eq!(
            domain_uuid.unwrap_err().to_string(),
            "Incorrect domain UUID length: 33",
        );
    }

    #[test]
    fn parse_domain_caps() {
        // Hexadecimal digits in domain UUIDs must always be lowercase.
        let domain = "1234567890ABCDEF1234567890ABCDEF";

        let domain_uuid = DomainUuid::parse(domain);

        assert!(domain_uuid.is_err());
        assert_eq!(
            domain_uuid.unwrap_err().to_string(),
            "Invalid hex characters: \"1234567890ABCDEF1234567890ABCDEF\"",
        );
    }

    #[test]
    fn hexify_string_short() {
        assert_eq!(hexify_string("hello"), "8656c6c6f6");
    }

    #[test]
    fn hexify_string_long() {
        assert_eq!(
            hexify_string("this long string is more than sixteen characters 🙂"),
            "4786963702c6f6e6760237472796e67602963702d6f6275602478616e602379687475656e60236861627163647562737020ff99928",
        );
    }

    #[test]
    fn unhexify_string_short() {
        assert_eq!(unhexify_string("8656c6c6f6").unwrap(), "hello");
    }

    #[test]
    fn unhexify_string_long() {
        let hex = "4786963702c6f6e6760237472796e67602963702d6f6275602478616e602379687475656e60236861627163647562737020ff99928";

        assert_eq!(
            unhexify_string(hex).unwrap(),
            "this long string is more than sixteen characters 🙂",
        );
    }

    #[test]
    fn unhexify_string_odd_length() {
        assert_eq!(
            unhexify_string("4786963").unwrap_err().to_string(),
            "Odd-length hex string",
        );
    }

    #[test]
    fn unhexify_string_non_hex() {
        assert_eq!(
            unhexify_string("nonhex").unwrap_err().to_string(),
            "Invalid hex character: \"n\"",
        );
    }

    #[test]
    fn unhexify_string_non_hex_long() {
        assert_eq!(
            unhexify_string("nonhexnonhexnonhexnonhexnonhexnonhexnonhexnonhex")
                .unwrap_err()
                .to_string(),
            "Invalid hex characters: \"nonhexnonhexnonhexnonhexnonhexno\"",
        );
    }

    #[test]
    fn unhexify_string_non_utf8() {
        assert_eq!(
            unhexify_string("c328").unwrap_err().to_string(),
            "Hex string represents invalid UTF-8",
        );
    }
}
