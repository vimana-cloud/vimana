//! Utilities for dealing with service names, component names, and pod names.
//!
//! These types of names have significant syntactic overlap,
//! so they share a single permissive parsing function: [`Name::parse`],
//! which cheaply parses anything that *might* be any one of them.
//!
//! Once parsed, use the conversion functions to validate a name as a particular type.
//! These are:
//! - [`ServiceName`]
//! - [`ComponentName`]
//! - [`PodName`]
#![feature(core_intrinsics)]

use std::fmt::{Display, Formatter, Result as FmtResult, Write};
use std::intrinsics::likely;

use lazy_static::lazy_static;
use regex::Regex;

use error::{Error, Result};

const DOMAIN_SEPARATOR: char = ':';
const VERSION_SEPARATOR: char = '@';
const POD_ID_SEPARATOR: char = '#';

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

    /// Return the domain, if one was explicitly provided.
    /// Otherwise, return the inferred default domain based on the service name.
    fn domain_or_default(&self) -> String {
        self.domain.map_or_else(
            || {
                self.service
                    .split('.')
                    .rev()
                    .skip(1)
                    .collect::<Vec<&str>>()
                    .join(".")
            },
            String::from,
        )
    }

    /// If the domain is explicitly present and valid,
    /// but the version and pod ID are absent,
    /// consume self and return an equivalent [`ServiceName`].
    pub fn full_service(self) -> Result<ServiceName> {
        if let Some(domain) = self.domain {
            if self.version.is_none() && self.pod.is_none() {
                return ServiceName::new(domain, self.service);
            }
        }
        Err(Error::leaf(FULL_SERVICE_ERROR_MSG))
    }

    /// If the domain (or a default inferred from the service name) is valid,
    /// but the version and pod ID are absent,
    /// consume self and return an equivalent [`ServiceName`].
    pub fn service(self) -> Result<ServiceName> {
        if self.version.is_none() && self.pod.is_none() {
            return ServiceName::new(self.domain_or_default(), self.service);
        }
        Err(Error::leaf(SERVICE_ERROR_MSG))
    }

    /// If the domain and version are both explicitly present and valid,
    /// but the pod ID is absent,
    /// consume self and return an equivalent [`ComponentName`].
    pub fn full_component(self) -> Result<ComponentName> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                if self.pod.is_none() {
                    return ComponentName::new(domain, self.service, version);
                }
            }
        }
        Err(Error::leaf(FULL_COMPONENT_ERROR_MSG))
    }

    /// If the version is explicitly present and valid,
    /// and the domain (or a default inferred from the service name) is valid,
    /// but the pod ID is absent,
    /// consume self and return an equivalent [`ComponentName`].
    pub fn component(self) -> Result<ComponentName> {
        if let Some(version) = self.version {
            if self.pod.is_none() {
                return ComponentName::new(&self.domain_or_default(), self.service, version);
            }
        }
        Err(Error::leaf(COMPONENT_ERROR_MSG))
    }

    /// If the domain, version, and pod ID are all explicitly present and valid,
    /// consume self and return an equivalent [`PodName`].
    /// Pod names are always fully-qualified.
    pub fn pod(self) -> Result<PodName> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                if let Some(pod) = self.pod {
                    return PodName::new(domain, self.service, version, pod);
                }
            }
        }
        Err(Error::leaf(POD_ERROR_MSG))
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

pub(crate) const FULL_SERVICE_ERROR_MSG: &str = "Expected fully-qualified service name.";
pub(crate) const SERVICE_ERROR_MSG: &str = "Expected service name.";
pub(crate) const FULL_COMPONENT_ERROR_MSG: &str = "Expected fully-qualified component name.";
pub(crate) const COMPONENT_ERROR_MSG: &str = "Expected component name.";
pub(crate) const POD_ERROR_MSG: &str = "Expected pod ID.";

/// A service name:
///     domain ':' service-name
#[derive(Debug, PartialEq)]
pub struct ServiceName {
    pub domain: String,
    pub service: String,
}

impl ServiceName {
    pub fn new<D, S>(domain: D, service: S) -> Result<Self>
    where
        D: Into<String>,
        S: Into<String>,
    {
        let domain = domain.into();
        if likely(is_valid_dotted_path(&domain)) {
            let service = service.into();
            if likely(is_valid_dotted_path(&service)) {
                Ok(Self { domain, service })
            } else {
                Err(Error::leaf(format!("Invalid service name: {service}")))
            }
        } else {
            Err(Error::leaf(format!("Invalid domain: {domain}")))
        }
    }
}

impl Display for ServiceName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str(&self.domain)?;
        formatter.write_char(DOMAIN_SEPARATOR)?;
        formatter.write_str(&self.service)
    }
}

/// A component name (versioned service):
///     domain ':' service-name '@' version
#[derive(Debug, PartialEq)]
pub struct ComponentName {
    pub service: ServiceName,
    pub version: String,
}

impl ComponentName {
    pub fn new<D, S, V>(domain: D, service: S, version: V) -> Result<Self>
    where
        D: Into<String>,
        S: Into<String>,
        V: Into<String>,
    {
        let version = version.into();
        if likely(is_valid_version(&version)) {
            Ok(Self {
                service: ServiceName::new(domain, service)?,
                version,
            })
        } else {
            Err(Error::leaf(format!("Invalid version: {version}")))
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
#[derive(Debug, PartialEq)]
pub struct PodName {
    pub component: ComponentName,
    pub pod: usize,
}

impl PodName {
    pub fn new<'a, D, S, V>(domain: D, service: S, version: V, pod: &'a str) -> Result<Self>
    where
        D: Into<String>,
        S: Into<String>,
        V: Into<String>,
    {
        Ok(Self {
            component: ComponentName::new(domain, service, version)?,
            pod: usize::from_str_radix(pod, 16)
                .map_err(|_e| Error::leaf(format!("Invalid pod ID: {pod}")))?,
        })
    }
}

impl Display for PodName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        self.component.fmt(formatter)?;
        formatter.write_char(POD_ID_SEPARATOR)?;
        formatter.write_fmt(format_args!("{:X}", self.pod))
    }
}

/// Maximum length (inclusive) for K8s labels values.
const K8S_LABEL_VALUE_MAX_LENGTH: usize = 63;

/// Predicate for valid domain and service names
///
/// True if `name` contains at least two non-empty parts separated by dots (`.`).
/// Domains must have at least two parts because TLDs are not allowed.
/// Service names must have at least two parts because they're prefixed by a package.
/// In addition, because both domains and service names are stored as
/// [K8s labels](https://kubernetes.io/docs/concepts/overview/working-with-objects/labels/#syntax-and-character-set),
/// their max length is 63 each,
/// and they can only contain ASCII alphanumerics, dashes, and underscores.
/// They must start and end with alphanumerics.
fn is_valid_dotted_path(name: &str) -> bool {
    lazy_static! {
        static ref PATH_RE: Regex = Regex::new(concat!(
            r"^[0-9A-Za-z][0-9A-Za-z_-]*\.",  // first part and dot
            r"(?:[0-9A-Za-z_-]+\.)*",         // middle parts and dots
            r"[0-9A-Za-z_-]*[0-9A-Za-z]$",    // final part
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

    fn parse_and_convert_all(
        name: &str,
    ) -> (
        Result<ServiceName>,
        Result<ServiceName>,
        Result<ComponentName>,
        Result<ComponentName>,
        Result<PodName>,
    ) {
        (
            Name::parse(name).full_service(),
            Name::parse(name).service(),
            Name::parse(name).full_component(),
            Name::parse(name).component(),
            Name::parse(name).pod(),
        )
    }

    #[test]
    fn full_service() {
        let (full_service_name, service_name, full_component_name, component_name, pod_name) =
            parse_and_convert_all("ex_am-ple.c0m:some.Service");

        assert!(full_service_name.is_ok());
        let full_service_name = full_service_name.unwrap();
        assert_eq!(full_service_name.domain, "ex_am-ple.c0m");
        assert_eq!(full_service_name.service, "some.Service");
        assert_eq!(format!("{full_service_name}"), "ex_am-ple.c0m:some.Service");

        assert!(service_name.is_ok());
        assert_eq!(service_name.unwrap(), full_service_name);

        assert!(full_component_name.is_err());
        assert_eq!(
            full_component_name.unwrap_err().msg,
            FULL_COMPONENT_ERROR_MSG
        );
        assert!(component_name.is_err());
        assert_eq!(component_name.unwrap_err().msg, COMPONENT_ERROR_MSG);
        assert!(pod_name.is_err());
        assert_eq!(pod_name.unwrap_err().msg, POD_ERROR_MSG);
    }

    #[test]
    fn full_component() {
        let (full_service_name, service_name, full_component_name, component_name, pod_name) =
            parse_and_convert_all(
                "this.is.just.under.sixty.three.characters.which.is.the.maximum:some.Service@1.0.0",
            );

        assert!(full_component_name.is_ok());
        let full_component_name = full_component_name.unwrap();
        assert_eq!(
            full_component_name.service.domain,
            "this.is.just.under.sixty.three.characters.which.is.the.maximum"
        );
        assert_eq!(full_component_name.service.service, "some.Service");
        assert_eq!(full_component_name.version, "1.0.0");
        assert_eq!(
            format!("{full_component_name}"),
            "this.is.just.under.sixty.three.characters.which.is.the.maximum:some.Service@1.0.0"
        );

        assert!(component_name.is_ok());
        assert_eq!(component_name.unwrap(), full_component_name);

        assert!(full_service_name.is_err());
        assert_eq!(full_service_name.unwrap_err().msg, FULL_SERVICE_ERROR_MSG);
        assert!(service_name.is_err());
        assert_eq!(service_name.unwrap_err().msg, SERVICE_ERROR_MSG);
        assert!(pod_name.is_err());
        assert_eq!(pod_name.unwrap_err().msg, POD_ERROR_MSG);
    }

    #[test]
    fn pod() {
        let (full_service_name, service_name, full_component_name, component_name, pod_name) =
            parse_and_convert_all("example.com:some.Service@1.0.0#19AF0");

        assert!(pod_name.is_ok());
        let pod_name = pod_name.unwrap();
        assert_eq!(pod_name.component.service.domain, "example.com");
        assert_eq!(pod_name.component.service.service, "some.Service");
        assert_eq!(pod_name.component.version, "1.0.0");
        assert_eq!(pod_name.pod, 0x19af0usize);
        assert_eq!(
            format!("{pod_name}"),
            "example.com:some.Service@1.0.0#19AF0"
        );

        assert!(full_service_name.is_err());
        assert_eq!(full_service_name.unwrap_err().msg, FULL_SERVICE_ERROR_MSG);
        assert!(service_name.is_err());
        assert_eq!(service_name.unwrap_err().msg, SERVICE_ERROR_MSG);
        assert!(full_component_name.is_err());
        assert_eq!(
            full_component_name.unwrap_err().msg,
            FULL_COMPONENT_ERROR_MSG
        );
        assert!(component_name.is_err());
        assert_eq!(component_name.unwrap_err().msg, COMPONENT_ERROR_MSG);
    }

    #[test]
    fn service() {
        let (full_service_name, service_name, full_component_name, component_name, pod_name) =
            parse_and_convert_all("com.example.Service");

        assert!(service_name.is_ok());
        let service_name = service_name.unwrap();
        assert_eq!(service_name.domain, "example.com");
        assert_eq!(service_name.service, "com.example.Service");

        assert!(full_service_name.is_err());
        assert_eq!(full_service_name.unwrap_err().msg, FULL_SERVICE_ERROR_MSG);

        assert!(full_component_name.is_err());
        assert!(component_name.is_err());
        assert!(pod_name.is_err());
    }

    #[test]
    fn component() {
        let (full_service_name, service_name, full_component_name, component_name, pod_name) =
            parse_and_convert_all("com.example.Service@0.0.123--0-fersher");

        assert!(component_name.is_ok());
        let component_name = component_name.unwrap();
        assert_eq!(component_name.service.domain, "example.com");
        assert_eq!(component_name.service.service, "com.example.Service");
        assert_eq!(component_name.version, "0.0.123--0-fersher");

        assert!(full_component_name.is_err());
        assert_eq!(
            full_component_name.unwrap_err().msg,
            FULL_COMPONENT_ERROR_MSG
        );

        assert!(full_service_name.is_err());
        assert!(service_name.is_err());
        assert!(pod_name.is_err());
    }

    #[test]
    fn bad_domain() {
        let bad_domains = vec![
            "tld",
            "_starts.underscore",
            "ends.dash-",
            "this.is.longer.than.sixty.three.characters.which.is.the.maximum.allowed",
        ];
        for domain in bad_domains.iter() {
            let name = format!("{domain}:some.Service");
            let service_name = Name::parse(&name).service();

            assert!(service_name.is_err());
            assert_eq!(
                service_name.unwrap_err().msg,
                format!("Invalid domain: {domain}")
            );
        }
    }

    #[test]
    fn bad_default_domain() {
        // The default domain would be a TLD in this case.
        let service_name = Name::parse("com.Service").service();

        assert!(service_name.is_err());
        assert_eq!(service_name.unwrap_err().msg, "Invalid domain: com");
    }

    #[test]
    fn bad_service_name() {
        let bad_services = vec!["NoPackage", "_package.StartsUnderscore"];
        for service in bad_services.iter() {
            let name = format!("example.com:{service}");
            let service_name = Name::parse(&name).service();

            assert!(service_name.is_err());
            assert_eq!(
                service_name.unwrap_err().msg,
                format!("Invalid service name: {service}")
            );
        }
    }

    #[test]
    fn bad_version() {
        let bad_versions = vec![
            "1.0.00",
            "1.2.3+build",
            "1.2.3-ends-dash-",
            "1.2.3-00",
            "1234567890.1234567890.1234567890-ABCDEFGHIJKLMNOPQRSTUVWXYZ-abcdefghijklmnopqrstuvwxyz",
        ];
        for version in bad_versions.iter() {
            let name = format!("example.com:some.Service@{version}");
            let component_name = Name::parse(&name).component();

            assert!(component_name.is_err());
            assert_eq!(
                component_name.unwrap_err().msg,
                format!("Invalid version: {version}")
            );
        }
    }

    #[test]
    fn good_version() {
        let good_versions = vec![
            "1.0.0",
            "1.2.3-pre-release",
            "1.2.3-0",
            "1.2.3-123",
            "1.2.3-00a",
        ];
        for version in good_versions.iter() {
            let name = format!("example.com:some.Service@{version}");
            assert!(Name::parse(&name).component().is_ok());
        }
    }

    #[test]
    fn bad_pod_id() {
        let bad_pod_ids = vec!["abcdefg", "-1", "10000000000000000"];
        for pod_id in bad_pod_ids.iter() {
            let name = format!("example.com:some.Service@1.2.3#{pod_id}");
            let pod_name = Name::parse(&name).pod();

            assert!(pod_name.is_err());
            assert_eq!(
                pod_name.unwrap_err().msg,
                format!("Invalid pod ID: {pod_id}")
            );
        }
    }

    #[test]
    fn good_pod_id() {
        let good_pod_ids = vec!["abcdefff", "0", "FFFFFFFFFFFFFFFF"];
        for pod_id in good_pod_ids.iter() {
            let name = format!("example.com:some.Service@1.2.3#{pod_id}");
            assert!(Name::parse(&name).pod().is_ok());
        }
    }
}
