# Security policy

safexip is an internet-facing authoritative DNS server with an authenticated
ACME challenge API. Please report suspected vulnerabilities privately.

## Supported versions

Security fixes are provided for the latest released version. Upgrade to the
newest release before reporting an issue that may already be fixed.

## Reporting a vulnerability

Use GitHub's **Report a vulnerability** button on this repository's Security
page. Do not open a public issue, discussion, or pull request containing exploit
details, API keys, ACME account data, certificate private keys, or production
deployment information.

Include the affected version, installation method, impact, reproduction steps,
and any suggested mitigation. Use placeholder domains and credentials wherever
possible. You should receive an acknowledgement within seven days. Please allow
time for a fix and coordinated release before public disclosure.

## Deployment boundary

HTTP Basic authentication does not provide transport encryption. Keep the API
on loopback or a private network, or place it behind verified HTTPS as described
in the [production guide](docs/production.md). safexip does not store ACME
accounts, certificate private keys, or issued certificates.
