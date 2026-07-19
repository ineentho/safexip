# Release documentation checklist

Complete this checklist before merging the release PR. `release-plz` creates the public release tag from that merge. The release workflow repeats the automated checks from the tagged source snapshot; the manual smoke tests catch host, firewall, DNS, and upgrade behavior that an isolated CI fixture cannot reproduce.

## Versioned artifacts

- [ ] Confirm the release PR updated the version in `Cargo.toml` and `Cargo.lock` and updated `CHANGELOG.md`.
- [ ] Confirm `deploy/compose.yml` uses `ineentho/safexip:latest` and the production guide documents how to pin a versioned tag.
- [ ] Confirm `docs/production.md`, `deploy/compose.yml`, and both initialization helpers are committed before merging.
- [ ] Run `make check` and `scripts/validate-production-docs.sh`.

## Clean-environment validation

On a disposable supported Ubuntu host:

- [ ] Follow every first-install shell block in `docs/production.md` using documentation values in a test zone.
- [ ] Supply a verified candidate/release image digest and run `sudo docker compose config --quiet` and `pull`.
- [ ] Verify direct UDP and TCP DNS before delegation.
- [ ] Verify public delegation, HTTPS trust, HTTP-to-HTTPS redirect, failed authentication, and one staging end-to-end lego issuance.
- [ ] Record the tested Ubuntu, Docker, Compose, Traefik, lego, and safexip versions in the release notes.

## Existing-deployment validation

On a disposable clone or backup-restored deployment containing sentinel ACME and credential state:

- [ ] Hash `.env`, `safexip.env`, `letsencrypt/acme.json`, the employee API-key file, and the employee lego directory.
- [ ] Rerun both initialization helpers and confirm they report preservation.
- [ ] Confirm every hash is unchanged.
- [ ] Follow the upgrade procedure and verify only the safexip image changes.
- [ ] Restart both services and complete the verification checklist and a staging lego renewal/run.
- [ ] Restore the deployment from backup and repeat the verification checklist.

Do not execute a destructive rotation procedure merely to validate an ordinary release. Test rotation separately on disposable credentials when that procedure changes.

## Publication

- [ ] Merge the release PR only after the guide and deployment files are in the commit that `release-plz` will tag.
- [ ] After the workflow publishes the multi-platform image, publish its verified digest with the release and substitute it when testing the final Compose deployment.
- [ ] Confirm the GitHub release links to the production guide in the matching source tag.
