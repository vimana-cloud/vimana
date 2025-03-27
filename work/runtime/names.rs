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

use std::fmt::{Display, Formatter, Result as FmtResult, Write};
use std::simd::cmp::SimdPartialOrd;
use std::simd::{simd_swizzle, u8x16, u8x32};
use std::str;

use lazy_static::lazy_static;
use regex::Regex;
use tonic::{Code, Status};

use error::{log_error_status, Result};

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

pub type PodId = usize;

/// Permissively [parses](Self::parse) a service / version name:
///     [ domain ':' ] service-name [ '@' version [ '#' pod-id ] ]
///
/// Use the conversion functions to validate and convert to particular name types.
pub struct Name<'a> {
    pub domain: Option<&'a str>,
    pub service: &'a str,
    pub version: Option<&'a str>,
    pub pod: Option<&'a str>,
}

impl<'a> Name<'a> {
    /// Permissively parse a service / version name:
    ///     [ domain ':' ] service-name [ '@' version [ '#' pod-id ] ]
    pub fn parse(full: &'a str) -> Self {
        let mut domain: Option<&'a str> = None;
        let mut version: Option<&'a str> = None;
        let mut pod: Option<&'a str> = None;

        // Used to mark the boundaries of the service name part:
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
        let service = &full[start..end];

        Name {
            domain,
            service,
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
                    return ComponentName::new(DomainUuid::parse(domain)?, self.service, version);
                }
            }
        }
        Err(Status::invalid_argument("invalid-component-name"))
    }

    /// If the domain, version, and pod ID are all explicitly present and valid,
    /// consume self and return an equivalent [`PodName`].
    /// Pod names are always fully-qualified.
    pub fn pod(self) -> Result<PodName> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                if let Some(pod) = self.pod {
                    let component =
                        ComponentName::new(DomainUuid::parse(domain)?, self.service, version)?;
                    let pod = usize::from_str_radix(pod, 16).map_err(log_error_status!(
                        Code::InvalidArgument,
                        "invalid-pod-id",
                        &component
                    ))?;
                    return Ok(PodName::new(component, pod));
                }
            }
        }
        Err(Status::invalid_argument("invalid-pod-name"))
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
        if uuid.len() < 32 {
            return Err(Status::invalid_argument("domain-uuid-too-short"));
        }
        if uuid.len() > 32 {
            return Err(Status::invalid_argument("domain-uuid-too-long"));
        }
        let hex_bytes = u8x32::from_slice(uuid.as_bytes());

        // Check that nothing is outside the range `[0-f]` or inside `[9-a]`.
        if hex_bytes.simd_lt(ASCII_ZERO_32).any()
            || hex_bytes.simd_gt(ASCII_F_32).any()
            || (hex_bytes.simd_gt(ASCII_NINE_32) & hex_bytes.simd_lt(ASCII_A_32)).any()
        {
            return Err(Status::invalid_argument("domain-uuid-invalid-characters"));
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
        Ok(Self {
            uuid: lower_nibbles + (upper_nibbles * SIXTEEN_16),
        })
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
        let upper_nibbles = self.uuid / SIXTEEN_16;
        let lower_nibbles = self.uuid % SIXTEEN_16;
        let nibbles = simd_swizzle!(
            upper_nibbles,
            lower_nibbles,
            [
                16, 0, 17, 1, 18, 2, 19, 3, 20, 4, 21, 5, 22, 6, 23, 7, 24, 8, 25, 9, 26, 10, 27,
                11, 28, 12, 29, 13, 30, 14, 31, 15,
            ],
        );
        let hex_bytes = nibbles
            + nibbles
                .simd_gt(NINE_32)
                .select(ASCII_A_FROM_TEN_32, ASCII_ZERO_32);
        let array = hex_bytes.as_array();
        let uuid = unsafe { str::from_utf8_unchecked(array) };
        formatter.write_str(uuid)
    }
}

/// A service name:
///     domain ':' service-name
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServiceName {
    pub domain: DomainUuid,
    pub service: String,
}

impl ServiceName {
    pub fn new<S>(domain: DomainUuid, service: S) -> Result<Self>
    where
        S: Into<String>,
    {
        let service = service.into();
        if is_valid_service_name(&service) {
            Ok(Self { domain, service })
        } else {
            Err(Status::invalid_argument("invalid-service-name"))
        }
    }
}

impl Display for ServiceName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        self.domain.fmt(formatter)?;
        formatter.write_char(DOMAIN_SEPARATOR)?;
        formatter.write_str(&self.service)
    }
}

/// A component name (versioned service):
///     domain ':' service-name '@' version
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentName {
    pub service: ServiceName,
    pub version: String,
}

impl ComponentName {
    pub fn new<S, V>(domain: DomainUuid, service: S, version: V) -> Result<Self>
    where
        S: Into<String>,
        V: Into<String>,
    {
        let version = version.into();
        if is_valid_version(&version) {
            Ok(Self {
                service: ServiceName::new(domain, service)?,
                version,
            })
        } else {
            Err(Status::invalid_argument("invalid-version"))
        }
    }
}

impl Display for ComponentName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        self.service.fmt(formatter)?;
        formatter.write_char(VERSION_SEPARATOR)?;
        formatter.write_str(&self.version)
    }
}

/// A pod / container name:
///     domain ':' service-name '@' version '#' pod-id
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
        self.component.fmt(formatter)?;
        formatter.write_char(POD_ID_SEPARATOR)?;
        formatter.write_fmt(format_args!("{:x}", self.pod))
    }
}

/// Maximum length (inclusive) for K8s labels values.
const K8S_LABEL_VALUE_MAX_LENGTH: usize = 63;

/// Predicate for syntactically valid service names.
/// These are stored as
/// [K8s labels](https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/#syntax-and-character-set),
/// which dictates all constraints unless otherwise noted.
///
/// True iff `name`:
///   - Contains no more than 63 bytes.
///   - Consists of only ASCII alphanumerics and underscores.
///   - Starts and ends with alphanumerics.
///   - Contains at least two non-empty parts separated by dots (`.`),
///     (because full service names always have a package prefix).
///   - Each part cannot start with a digit
///     (because Protobuf disallows it).
fn is_valid_service_name(name: &str) -> bool {
    lazy_static! {
        static ref PATH_RE: Regex = Regex::new(concat!(
            r"^[A-Za-z][0-9A-Za-z_]*\.",                         // first part and dot
            r"(?:[A-Za-z_][0-9A-Za-z_]*\.)*",                    // middle parts and dots
            r"(?:[A-Za-z_][0-9A-Za-z_]*[0-9A-Za-z]|[A-Za-z])$",  // final part
        ))
        .unwrap();
    }
    name.len() <= K8S_LABEL_VALUE_MAX_LENGTH && PATH_RE.is_match(name)
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
    version.len() < K8S_LABEL_VALUE_MAX_LENGTH && VERSION_RE.is_match(version)
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
    const GOOD_SERVICE: &str = "abc._._1.Ser_vicE";
    const GOOD_VERSION: &str = "10.0.456-pre-release5";
    const GOOD_POD_ID: &str = "a10f0";
    const GOOD_COMPONENT: &str =
        "1234567890abcdef0f9ed8c7654b32a1:abc._._1.Ser_vicE@10.0.456-pre-release5";
    const GOOD_POD: &str =
        "1234567890abcdef0f9ed8c7654b32a1:abc._._1.Ser_vicE@10.0.456-pre-release5#a10f0";

    #[test]
    fn parse_component() {
        let component = Name::parse(GOOD_COMPONENT).component();

        assert!(component.is_ok());
        let component = component.unwrap();
        assert_eq!(
            component.service.domain,
            DomainUuid::parse(GOOD_DOMAIN).unwrap(),
        );
        assert_eq!(component.service.service, GOOD_SERVICE);
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
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVICE}@{GOOD_VERSION}#{pod_id_str}");

            let pod_name = Name::parse(&name).pod();

            assert!(pod_name.is_ok());
            let pod_name = pod_name.unwrap();
            assert_eq!(
                pod_name.component.service.domain,
                DomainUuid::parse(GOOD_DOMAIN).unwrap(),
            );
            assert_eq!(pod_name.component.service.service, GOOD_SERVICE);
            assert_eq!(pod_name.component.version, GOOD_VERSION);
            assert_eq!(&pod_name.pod, pod_id);
            assert_eq!(format!("{pod_name}"), name);
        }
    }

    #[test]
    fn parse_pod_ids_bad() {
        let bad_pod_ids = vec!["abcdefg", "-1", "10000000000000000"];

        for pod_id in bad_pod_ids.iter() {
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVICE}@{GOOD_VERSION}#{pod_id}");

            let pod_name = Name::parse(&name).pod();

            assert!(pod_name.is_err());
            assert_eq!(pod_name.unwrap_err().message(), "invalid-pod-id",);
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
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVICE}@{version}");

            let component = Name::parse(&name).component();

            assert!(component.is_ok());
            let component = component.unwrap();
            assert_eq!(
                component.service.domain,
                DomainUuid::parse(GOOD_DOMAIN).unwrap(),
            );
            assert_eq!(component.service.service, GOOD_SERVICE);
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
            let name = format!("{GOOD_DOMAIN}:{GOOD_SERVICE}@{version}");
            let component = Name::parse(&name).component();

            assert!(component.is_err());
            assert_eq!(component.unwrap_err().message(), "invalid-version");
        }
    }

    #[test]
    fn parse_services_bad() {
        let bad_services = vec![
            "NoPackage",
            "_package.StartsWithUnderscore",
            "this.service.name.would.be.too.long.due.to.being.over.SixtyThree",
        ];

        for service in bad_services.iter() {
            let name = format!("{GOOD_DOMAIN}:{service}@{GOOD_VERSION}");

            let component = Name::parse(&name).component();

            assert!(component.is_err());
            assert_eq!(component.unwrap_err().message(), "invalid-service-name");
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
        assert_eq!(domain_uuid.unwrap_err().message(), "domain-uuid-too-short");
    }

    #[test]
    fn parse_domain_long() {
        // 33 characters instead of 32.
        let domain = "1234567890abcdef1234567890abcdef1";

        let domain_uuid = DomainUuid::parse(domain);

        assert!(domain_uuid.is_err());
        assert_eq!(domain_uuid.unwrap_err().message(), "domain-uuid-too-long");
    }

    #[test]
    fn parse_domain_caps() {
        // Hexadecimal digits in domain UUIDs must always be lowercase.
        let domain = "1234567890ABCDEF1234567890ABCDEF";

        let domain_uuid = DomainUuid::parse(domain);

        assert!(domain_uuid.is_err());
        assert_eq!(
            domain_uuid.unwrap_err().message(),
            "domain-uuid-invalid-characters"
        );
    }
}
