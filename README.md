# pingerer

The command line tool for [Pingerer](https://pingerer.com): check your sites'
status, add sites and monitors, handle incidents, ping heartbeats and keep
alerts quiet while you deploy, from your terminal or CI. One small program
for macOS, Linux and Windows, with nothing else to install.

## Install

macOS or Linux, with Homebrew:

```sh
brew install pingerer-app/tap/pingerer
```

Or with the installer script (macOS and Linux):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pingerer-app/pingerer/releases/latest/download/pingerer-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/pingerer-app/pingerer/releases/latest/download/pingerer-installer.ps1 | iex"
```

Or download the file for your computer from the
[releases page](https://github.com/pingerer-app/pingerer/releases).

## Sign in

Create an API token on Pingerer's **API tokens** page (choose *Read only* for
dashboards and status checks, *Read & write* to make changes), then:

```sh
pingerer login            # asks for the token, checks it, saves it
pingerer whoami
```

The token is saved in `~/.config/pingerer/config.json` (or
`$XDG_CONFIG_HOME/pingerer/config.json`), readable only by you. In CI, set
`PINGERER_TOKEN` instead; `PINGERER_URL` points at another server
(default `https://pingerer.com`). `pingerer logout` forgets the token.

## Accounts

Commands use your first team account unless you pick one:

```sh
pingerer accounts
pingerer use "Your Bakery"      # from now on
pingerer status --account 42    # just this once
```

## Examples

```sh
pingerer status                                   # exits 3 when something is down
pingerer sites
pingerer sites add yourbakery.com --checks http,ssl,domain --every 1m
pingerer sites show yourbakery.com
pingerer sites pause yourbakery.com
pingerer sites rm yourbakery.com --yes

pingerer monitors --site yourbakery.com
pingerer monitors test mon_8xKq2                  # one check now; exits 3 if it fails
pingerer monitors set mon_3Hq7 --renews-on 2027-03-02   # for .kw, .sa, .ae, .io... (none to clear)
pingerer results mon_8xKq2 --since 24h --failed

pingerer health yourbakery.com                    # score and what to fix; exits 3 on a critical problem
pingerer health yourbakery.com --now              # check again now and wait for the result
pingerer link-preview yourbakery.com/menu         # how the page looks when shared; exits 3 if broken

pingerer incidents --open
pingerer incidents show INC-0042
pingerer incidents ack INC-0042
pingerer incidents note INC-0042 Restarted the web server
pingerer incidents resolve INC-0042 -m "Fixed by the host"

pingerer heartbeats add "Nightly backup" --every 1d --grace 30m
pingerer heartbeats ping "Nightly backup"
pingerer heartbeats ping 7Kd2xQ9aTestTokn --fail  # by token, no login needed

pingerer status-pages list
pingerer status-pages show your-bakery
pingerer updates post your-bakery --state investigating --title "Checkout errors" \
  --message "We're looking into it." --component "Online shop=partial_outage"
pingerer updates post your-bakery --state resolved --message "Fixed." --follow-up su_8Fk2

pingerer quiet-hours
pingerer quiet-hours set --on --from 22:00 --to 07:00 --time-zone Asia/Kuwait --weekends
pingerer quiet-hours set --weekend fri_sat        # or auto for your time zone's usual weekend

pingerer weather                              # which hosts and CDNs are having trouble now
pingerer weather cloudflare                   # exits 3 when Cloudflare is having trouble

pingerer clients list                         # an agency's client workspaces
pingerer clients add "Harbor Dental"
pingerer --account acct_7Hd2k sites add harbordental.com
```

Every command takes `--json` for machine-readable output, `--account`, `--url`
and `--[no-]color`. Color is used on a terminal unless `NO_COLOR` is set.
`pingerer help` and `pingerer <command> --help` list the rest.

### In CI

```sh
export PINGERER_TOKEN=...   # a read only token is enough
pingerer status || echo "Something is down"
```

Around a deploy (a read and write token):

```sh
id=$(pingerer maintenance start --sites yourbakery.com,shop.yourbakery.com --for 15m --title Deploy --json | jq -r .id)
./deploy.sh
pingerer wait-until-up yourbakery.com shop.yourbakery.com --timeout 5m   # exits 3 if they don't come back
pingerer maintenance end "$id"
```

During a maintenance window checks keep running but nothing alerts and the
time doesn't count against uptime; it ends on its own after `--for`.
`pingerer check <site-or-monitor>...` runs each uptime check (http, tcp and
ping monitors) once, now, and exits 3 if one fails; `wait-until-up` repeats
it every `--interval` (15s) until they pass. `pingerer sites pause` and
`resume` stop and start the checks altogether. On GitHub, the
[Pingerer action](https://github.com/pingerer-app/action) does the same.

Exit codes: `0` ok, `1` an error (network, API, not found), `2` wrong usage,
`3` something is down (or, for `health`, `link-preview` and `weather <company>`,
something needs your attention).

Quiet hours hold back alerts that can wait (SSL, domain, DNS, email setup and
slowness) overnight, and all weekend with `--weekends`. Downtime and missed
heartbeats always come straight through.

## Changes

- **0.3.0**: rewritten in Rust as one small program for macOS, Linux and
  Windows, with the same commands, options, output and settings as before
  (an existing `~/.config/pingerer/config.json` keeps working).
- **0.2.0**: `health` (a site's health score, and `--now` to check again),
  `link-preview`, `weather`, `quiet-hours`, and `monitors set` (including a
  domain's `--renews-on` date for registries that keep it private).
  `incidents show` says when the host is the likely cause, and
  `status-pages show` lists the page's badges.
- **0.1.0**: the first version.

## Development

```sh
cargo test           # the commands against a fake Pingerer, and every API call against openapi.yaml
cargo run -- help
```

Releases: bump `version` in Cargo.toml, then tag it (`git tag v0.3.1 && git
push --tags`). GitHub Actions builds every platform's download, the
installers and the Homebrew formula (`.github/workflows/release.yml`, made
by [dist](https://opensource.axo.dev/cargo-dist/)). The API itself is
documented at https://pingerer.com/docs/api.
