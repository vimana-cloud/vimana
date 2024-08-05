
/// Permissively [parses](Self::parse) a service / version name:
///     [ domain ':' ] service-name [ '@' version ]
/// and provides access to components.
pub struct ServiceName<'a> {

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

impl<'a> ServiceName<'a> {

    /// Parse a full service name of the form `<domain>:<service>@version`.
    pub fn parse(full: &'a str) -> Self {
        let mut start = 0;
        let mut end = full.len();

        // Optional domain.
        let domain = full.find(':').map( |index| {
            start = index + 1;
            &full[.. index]
        });
        // Optional version number.
        let version = full[start..].find('@').map( |index| {
            end = start + index;
            &full[end + 1 ..]
        });
        // Service name.
        let service = &full[start .. end];
        // Optional domain and `:`, followed by service name.
        let without_version = &full[.. end];
        // Service name, optionally followed by `@` and version number.
        let without_domain = &full[start ..];

        ServiceName { domain, service, version, without_version, without_domain }
    }

    pub fn domain_or_default(&self) -> String {
        self.domain.map_or_else(
          || self.service.split('.').rev().skip(1).collect::<Vec<&str>>().join("."),
          String::from,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full() {
        let parsed = ServiceName::parse("example.com:some.Service@1.0");

        assert_eq!(parsed.domain, Some("example.com"));
        assert_eq!(parsed.service, "some.Service");
        assert_eq!(parsed.version, Some("1.0"));
        assert_eq!(parsed.without_version, "example.com:some.Service");
        assert_eq!(parsed.without_domain, "some.Service@1.0");
        assert_eq!(parsed.domain_or_default(), "example.com".to_string());
    }

    #[test]
    fn parse_no_version() {
        let parsed = ServiceName::parse("weird-domain:some.Service");

        assert_eq!(parsed.domain, Some("weird-domain"));
        assert_eq!(parsed.service, "some.Service");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "weird-domain:some.Service");
        assert_eq!(parsed.without_domain, "some.Service");
        assert_eq!(parsed.domain_or_default(), "weird-domain".to_string());
    }

    #[test]
    fn parse_no_domain() {
        let parsed = ServiceName::parse("some.sort.of.Service@weird-version");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "some.sort.of.Service");
        assert_eq!(parsed.version, Some("weird-version"));
        assert_eq!(parsed.without_version, "some.sort.of.Service");
        assert_eq!(parsed.without_domain, "some.sort.of.Service@weird-version");
        assert_eq!(parsed.domain_or_default(), "of.sort.some".to_string());
    }

    #[test]
    fn parse_bare() {
        let parsed = ServiceName::parse("weird-service");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "weird-service");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "weird-service");
        assert_eq!(parsed.without_domain, "weird-service");
        assert_eq!(parsed.domain_or_default(), "".to_string());
    }

    #[test]
    fn parse_empty() {
        let parsed = ServiceName::parse("");

        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.service, "");
        assert_eq!(parsed.version, None);
        assert_eq!(parsed.without_version, "");
        assert_eq!(parsed.without_domain, "");
        assert_eq!(parsed.domain_or_default(), "".to_string());
    }
}
