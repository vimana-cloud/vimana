//! Standard name parsing / construction logic.

use error::{Error, Result};

/// Permissively [parse](Self::parse) a service / version name:
///     [ domain ':' ] service-name [ '@' version ]
///
/// Use the `to_*` functions to assert restrictions.
pub struct Name<'a> {
    /// Explicit domain;
    /// if empty, try [`domain_or_default`](Self::domain_or_default).
    pub domain: Option<&'a str>,

    /// Service name.
    pub service: &'a str,

    /// Version (permissive);
    /// if empty, this is an unversioned service name.
    pub version: Option<&'a str>,

    /// Service name, optionally preceded by a domain name and colon.
    pub without_version: &'a str,

    /// Service name, opitionally followed by a an at-sign and version.
    pub without_domain: &'a str,
}

impl<'a> Name<'a> {
    /// Permissively parse a service / version name:
    ///     [ domain ':' ] service-name [ '@' version ]
    pub fn parse(full: &'a str) -> Self {
        let mut start = 0;
        let mut end = full.len();

        let mut domain: Option<&'a str> = None;
        let mut version: Option<&'a str> = None;
        full.find(&[':', '@']).map(|index| {
            if full[index..].starts_with(':') {
                // Optional domain.
                domain = Some(&full[..index]);
                start = index + 1;
                version = full[start..].find('@').map(|v_index| {
                    // And optional version number.
                    end = start + v_index;
                    &full[end + 1..]
                });
            } else {
                // Just optional version number.
                end = index;
                version = Some(&full[end + 1..]);
            }
        });
        // Service name.
        let service = &full[start..end];
        // Optional domain and `:`, followed by service name.
        let without_version = &full[..end];
        // Service name, optionally followed by `@` and version number.
        let without_domain = &full[start..];

        Name {
            domain,
            service,
            version,
            without_version,
            without_domain,
        }
    }

    /// Return the domain, if one was explicitly provided.
    /// Otherwise, return the inferred default domain based on the service name.
    pub fn domain_or_default(&self) -> String {
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

    pub fn is_valid(&self) -> bool {
        is_valid_path(self.service)
            && self.domain.map_or(true, is_valid_path)
            && self.version.map_or(true, |version| version.len() > 0)
    }

    pub fn is_service(&self) -> bool {
        self.version.is_none()
    }

    pub fn is_version(&self) -> bool {
        self.version.is_some()
    }

    pub fn has_domain(&self) -> bool {
        self.domain.is_some()
    }

    pub fn to_full_version(self) -> Result<FullVersionName<'a>> {
        if let Some(domain) = self.domain {
            if let Some(version) = self.version {
                return Ok(FullVersionName {
                    domain: domain,
                    service: self.service,
                    version: version,
                    without_version: self.without_version,
                    without_domain: self.without_domain,
                });
            }
        }
        Err(Error::leaf(TO_FULL_VERSION_MSG))
    }
}

pub(crate) const TO_FULL_VERSION_MSG: &str = "Expected fully-qualified versioned service name.";

/// A fully-qualified service implementation name:
///     domain ':' service-name '@' version
#[derive(Debug)]
pub struct FullVersionName<'a> {
    /// Domain.
    pub domain: &'a str,

    /// Service name.
    pub service: &'a str,

    /// Version.
    pub version: &'a str,

    /// Domain name, colon, service name.
    pub without_version: &'a str,

    /// Service name, at-sign, version.
    pub without_domain: &'a str,
}

impl<'a> FullVersionName<'a> {
    pub fn is_valid(&self) -> bool {
        is_valid_path(self.service) && is_valid_path(self.domain) && self.version.len() > 0
    }
}

/// Return true iff `name` contains at least two non-empty parts separated by dots (`.`).
fn is_valid_path(name: &str) -> bool {
    let mut parts = name.split('.');
    match parts.next() {
        Some(first_part) => {
            // The first part must not be empty.
            first_part.len() > 0
                && match parts.next() {
                    Some(second_part) => {
                        // No subsequent part, if any, can be empty.
                        second_part.len() > 0 && parts.all(|part| part.len() > 0)
                    }
                    // A valid service name needs something after the package.
                    None => false,
                }
        }
        // A valid service name needs a package.
        // It must include at least one dot.
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full() {
        let parsed = Name::parse("example.com:some.Service@1.0");

        assert_eq!(parsed.domain, Some("example.com"));
        assert_eq!(parsed.service, "some.Service");
        assert_eq!(parsed.version, Some("1.0"));
        assert_eq!(parsed.without_version, "example.com:some.Service");
        assert_eq!(parsed.without_domain, "some.Service@1.0");
        assert_eq!(parsed.domain_or_default(), "example.com".to_string());
        assert!(parsed.is_valid());
    }

    #[test]
    fn parse_no_version() {
        let parsed = Name::parse("weird-domain:some.Service");

        assert_eq!(parsed.domain, Some("weird-domain"));
        assert_eq!(parsed.service, "some.Service");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "weird-domain:some.Service");
        assert_eq!(parsed.without_domain, "some.Service");
        assert_eq!(parsed.domain_or_default(), "weird-domain".to_string());
        // Not valid because the domain is weird (no dots).
        assert!(!parsed.is_valid());
    }

    #[test]
    fn parse_no_domain() {
        let parsed = Name::parse("some.sort.of.Service@weird-version");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "some.sort.of.Service");
        assert_eq!(parsed.version, Some("weird-version"));
        assert_eq!(parsed.without_version, "some.sort.of.Service");
        assert_eq!(parsed.without_domain, "some.sort.of.Service@weird-version");
        assert_eq!(parsed.domain_or_default(), "of.sort.some".to_string());
        // Valid because the version does not need dots.
        assert!(parsed.is_valid());
    }

    #[test]
    fn parse_bare() {
        let parsed = Name::parse("weird-service");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "weird-service");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "weird-service");
        assert_eq!(parsed.without_domain, "weird-service");
        assert_eq!(parsed.domain_or_default(), "".to_string());
        // Not valid because the service name is weird (no dots).
        assert!(!parsed.is_valid());
    }

    #[test]
    fn parse_empty() {
        let parsed = Name::parse("");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "");
        assert_eq!(parsed.without_domain, "");
        assert_eq!(parsed.domain_or_default(), "".to_string());
        assert!(!parsed.is_valid());
    }

    #[test]
    fn validation_empty_version() {
        let parsed = Name::parse("a.b:c.d@");

        assert!(parsed.is_version());
        assert!(!parsed.is_service());
        assert!(parsed.has_domain());
        assert!(!parsed.is_valid());
    }

    #[test]
    fn to_full_version_ok() {
        let parsed = Name::parse("example.com:foo.BarService@1.2-ok");
        assert!(parsed.is_valid()); // sanity check

        let full = parsed.to_full_version().unwrap();

        assert!(full.is_valid());
        assert_eq!(full.domain, "example.com");
        assert_eq!(full.service, "foo.BarService");
        assert_eq!(full.version, "1.2-ok");
    }

    #[test]
    fn to_full_version_no_domain() {
        let parsed = Name::parse("foo.BarService@1.2-ok");
        assert!(parsed.is_valid()); // sanity check

        let error = parsed.to_full_version().unwrap_err();

        assert_eq!(error.msg, TO_FULL_VERSION_MSG);
    }

    #[test]
    fn to_full_version_no_version() {
        let parsed = Name::parse("example.com:foo.BarService");
        assert!(parsed.is_valid()); // sanity check

        let error = parsed.to_full_version().unwrap_err();

        assert_eq!(error.msg, TO_FULL_VERSION_MSG);
    }
}
