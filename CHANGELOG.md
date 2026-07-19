# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2](https://github.com/ineentho/safexip/compare/v0.2.1...v0.2.2) - 2026-07-19

### Fixed

- harden TCP responses and release automation

### Security

- Bound TCP DNS response writes so slow clients cannot retain a connection indefinitely.

### Other

- Add vulnerability-reporting, issue, and repository-readiness guidance.
- Pin GitHub Actions, container bases, and the nFPM installer checksum, and configure weekly dependency updates.
- Prevent release automation from running twice and duplicating generated notes.

## [0.2.1](https://github.com/ineentho/safexip/compare/v0.2.0...v0.2.1) - 2026-07-19

### Added

- *(api)* enforce defense-in-depth HTTP request limits ([#17](https://github.com/ineentho/safexip/pull/17))
- *(deploy)* add non-destructive production setup ([#14](https://github.com/ineentho/safexip/pull/14))

### Fixed

- *(packaging)* fix Debian and Arch package smoke environments
- *(dns)* correct authoritative SOA and NS responses ([#16](https://github.com/ineentho/safexip/pull/16))
- *(dns)* enforce ACME TXT wire capacity ([#15](https://github.com/ineentho/safexip/pull/15))
- *(dns)* enforce query semantics and apex-only authority ([#13](https://github.com/ineentho/safexip/pull/13))
- *(packaging)* support static Alpine packages with OpenRC ([#12](https://github.com/ineentho/safexip/pull/12))

### Other

- *(release)* automate release PRs and Docker artifacts ([#18](https://github.com/ineentho/safexip/pull/18))
- *(license)* offer MIT or Apache-2.0 licensing ([#11](https://github.com/ineentho/safexip/pull/11))
- Document production deployment and ZeroSSL workflow
