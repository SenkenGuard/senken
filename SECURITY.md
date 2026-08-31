# Security policy

Senken stores user credentials, enforces per-user authorisation over market
and account data, and is heading toward executing trades. Security reports
are taken seriously here.

## Reporting a vulnerability

**Please do not open a public issue.**

Use [private vulnerability reporting](https://github.com/SenkenGuard/senken/security/advisories/new),
which opens a draft advisory visible only to you and the maintainers.

Useful things to include, if you have them: what an attacker gains, the
smallest set of steps that demonstrates it, the version or commit you tested,
and your operating system. A proof of concept is welcome but not required to
report something.

You can expect an acknowledgement within a few days. Because this project is
maintained by a small team, please allow reasonable time for a fix before
disclosing publicly — and tell us if you have a disclosure deadline in mind,
so we can plan against it rather than around it.

## What is in scope

- Authentication and session handling
- Authorisation — anything that lets one user reach another user's
  workspaces, layouts, alerts, accounts or broker credentials
- Storage of credentials and secrets
- Path handling in the data directory
- The HTTP and WebSocket surface, including the single-use WebSocket ticket
- Dependency vulnerabilities that are actually reachable from Senken's code

## What is not

- Vulnerabilities in a venue's own API or website
- Findings that require an attacker to already control the machine Senken
  runs on
- Missing hardening with no demonstrated impact, from an automated scanner
  and nothing else

## Supported versions

Senken is pre-1.0. Fixes land on `main` and go out in the next release; there
are no maintained release branches yet.
