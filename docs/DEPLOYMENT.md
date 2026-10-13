# Deployment guide (Phase 8): RajeshDB on a Linux server with HTTPS

**Honest status:** the Docker images, the Compose file and the Caddy configuration in `deploy/` were written
without access to Docker or a Rust compiler. YAML and shell syntax were checked; nothing was built or run.
Treat your first deployment as a rehearsal on a throw-away server, and keep a copy of this checklist's results.

## 1. What you are deploying

    Internet --443--> [ Caddy: TLS, dashboard files, /api -> platform ] --internal network--> [ rajeshdb-platform :8081 ]
                                                                                                        |
                                                                                           /srv/rajeshdb-data (data + backups)

* Only Caddy publishes ports (80, 443). The database API port 8081 is reachable only inside the Docker network.
* The dashboard is served by Caddy; the browser talks to `/api/...` on the same domain (no CORS, no extra port).
* `/api/ops/*` (operator endpoints) answers 404 to everyone except the IPs in `OPS_ALLOW_IPS`.
* The server runs as an unprivileged user (uid 10001), read-only root filesystem, no Linux capabilities.
* All data AND backups live in one folder on the host (`DATA_MOUNT`). That is your "persistent disk".

## 2. Where to run it (free for testing, paid for real use)

Never promise yourself "free forever": free tiers change without warning (see the table below).

| Option | Good for | Checked limits (2026, verify on the provider's page before relying on them) |
|---|---|---|
| Your own Windows laptop (`cargo run`, `npm run dev`) | development, always free | stops when the laptop is off |
| Oracle Cloud "Always Free" Ampere ARM VM | a free **staging/demo** server | Reported cut in June 2026 to 2 OCPU / 12 GB RAM; 200 GB block storage total; idle instances can be reclaimed (documented: 95th-percentile CPU under 20% over 7 days); capacity shortages; no SLA. Not suitable for production data |
| Small paid VPS (DigitalOcean, Hetzner, Vultr, Linode, AWS Lightsail, ...) | production start | see cost table; prices and availability have moved a lot in 2026 (Hetzner repriced twice and some cheap plans were marked unavailable), so compare current prices on the day you buy |

Pick a server with **at least 2 GB RAM**, Ubuntu 24.04 LTS, and a region close to your users (for India:
Mumbai/Bangalore/Singapore regions where the provider has them). x86 and ARM both work: the image builds on whatever
CPU it is built on.

## 3. Prepare the server (once)

1. Create the server with SSH key login (not password). Note its public IP.
2. Log in and update: `sudo apt update && sudo apt upgrade -y`
3. Firewall (allow only SSH, HTTP, HTTPS):

       sudo ufw allow OpenSSH
       sudo ufw allow 80/tcp
       sudo ufw allow 443/tcp
       sudo ufw allow 443/udp
       sudo ufw enable

   Also use the provider's cloud firewall if it has one. Do NOT open port 8081 or any database port.
4. Install Docker Engine + the Compose plugin by following the current official instructions for Ubuntu at
   https://docs.docker.com/engine/install/ubuntu/ (instructions change; do not copy old ones from elsewhere).
5. Optional but wise on a 2 GB machine, because compiling Rust is memory-hungry: add swap

       sudo fallocate -l 4G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile
       echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab

   Or skip compiling on the server: use the GitHub Actions image build (section 5b).
6. Enable automatic security updates: `sudo apt install -y unattended-upgrades`

## 4. Put the code on GitHub (from your Windows laptop)

First generate lock files so builds are reproducible (they pin exact dependency versions):

    cd D:\rajeshdb\engine
    cargo generate-lockfile
    cd ..\dashboard
    npm install            # creates package-lock.json

Then:

    cd D:\rajeshdb
    git init
    git add .
    git status             # check: no .env, no data folders, no target\ or node_modules\ in the list
    git commit -m "RajeshDB"
    # create an EMPTY private repository on github.com, then:
    git remote add origin https://github.com/YOUR_USER/rajeshdb.git
    git branch -M main
    git push -u origin main

`.github/workflows/ci.yml` then runs all Rust and dashboard tests on every push. Read its first run carefully: it is the
first time everything is compiled on a clean machine.

## 5. Deploy

### 5a. Build on the server
    git clone https://github.com/YOUR_USER/rajeshdb.git && cd rajeshdb/deploy
    cp .env.example .env
    nano .env                          # set SITE_ADDRESS, OPS_ALLOW_IPS, RAJESHDB_OPERATOR_TOKEN (openssl rand -hex 32)
    sudo mkdir -p /srv/rajeshdb-data && sudo chown 10001:10001 /srv/rajeshdb-data
    docker compose up -d --build
    docker compose ps                  # both services should become "healthy"/"running"
    docker compose logs -f platform

### 5b. Or pull a prebuilt image
Push a tag (`git tag v0.1.0 && git push origin v0.1.0`); `.github/workflows/docker-publish.yml` publishes
`ghcr.io/<your-user>/rajeshdb-platform`. In `docker-compose.yml` replace the `build:` block of `platform` with
`image: ghcr.io/<your-user>/rajeshdb-platform:v0.1.0` (log in with `docker login ghcr.io` if the package is private).

### 5c. Try it locally first (no domain needed)
On any machine with Docker: leave `SITE_ADDRESS=localhost` in `.env`, set `DATA_MOUNT=rajeshdb-data` (a Docker volume), run
`docker compose up -d --build`, open https://localhost (accept the self-signed certificate warning).

## 6. Domain and HTTPS

1. Buy a domain (any registrar). Create an **A record** (and AAAA for IPv6) for e.g. `console.example.com` pointing to the server IP.
2. Put the same name in `SITE_ADDRESS`. Ports 80 and 443 must be reachable from the internet.
3. `docker compose up -d`. Caddy requests and renews the Let's Encrypt certificate by itself. Watch `docker compose logs web`.
4. Check: `curl -I https://console.example.com/api/health` -> `200`, and `curl -I https://console.example.com/api/ops/metrics` -> `404`
   from any IP not listed in `OPS_ALLOW_IPS`.
5. Keep the `caddy_data` volume: it holds the certificates (deleting it repeatedly can hit Let's Encrypt rate limits).

## 7. First run

1. Open the site, **Register** your account, create an organization and a project.
2. If this is a private deployment, set `RAJESHDB_ALLOW_REGISTRATION=false` in `.env` and run `docker compose up -d` to apply.
   (Registration cannot be turned back on from the UI; edit `.env` again.)
3. Operations page -> paste your operator token -> "Run backups now" -> check the control backup appears.
4. Do a restore drill NOW, while there is nothing to lose: environment -> Backups -> Verify, then Restore.

## 8. Persistent disk checklist

* Data is in `DATA_MOUNT` on the host, outside the container: `docker compose down`, image rebuilds and updates do not touch it.
* Prove it: create a document, run `docker compose down && docker compose up -d`, read it back.
* Never run `docker compose down -v` if `DATA_MOUNT` is a Docker volume name: `-v` deletes volumes. (A host folder is not affected.)
* To use a separate provider "block volume" (recommended so that data survives rebuilding the server): attach it, format it
  once (`sudo mkfs.ext4 /dev/disk/by-id/...`, **only the first time**), mount it at `/srv/rajeshdb-data`, add it to `/etc/fstab`,
  `sudo chown 10001:10001 /srv/rajeshdb-data`. Provider steps differ; follow their docs.
* Watch free space: set `RAJESHDB_DISK_BUDGET_BYTES` BELOW the real disk size; the server alerts at 80% and 95% of that budget
  (it cannot see the physical disk's free space itself). Backups multiply storage: plan for data size x (1 + RAJESHDB_BACKUP_RETENTION).

## 9. Backups and disaster recovery

Automatic backups run inside the server (default every 6 h, 7 kept). They are on the SAME disk, so also copy them away:

    sudo apt install -y rsync
    # try it once by hand:
    RAJESHDB_OFFSITE_DEST="backupuser@other-host:/backups/rajeshdb" /path/to/rajeshdb/deploy/offsite-backup.sh
    # then schedule it (crontab -e):
    17 */6 * * * RAJESHDB_OFFSITE_DEST="backupuser@other-host:/backups/rajeshdb" /path/to/rajeshdb/deploy/offsite-backup.sh >> /var/log/rajeshdb-offsite.log 2>&1

`rclone:` destinations (S3, Backblaze B2, Google Drive, ...) are supported by the same script; use an rclone *crypt* remote so
the copies are encrypted (backups contain your data in clear text).

**If the server is lost** (new server, same domain):
1. Do sections 3-5 on a new server, but do not register yet.
2. Copy your off-site `backups` folder to `/srv/rajeshdb-data/backups` and `sudo chown -R 10001:10001 /srv/rajeshdb-data`.
3. Stop the stack, restore the control database from its newest backup (docs/OPERATIONS.md section 5), start the stack.
4. Log in with your old account; for each project environment: Backups -> Restore.
Rehearse this once before you need it. Everything written after the newest off-site copy is lost.

## 10. Updating

1. Run backups (Operations -> Run backups now) and make sure an off-site copy exists.
2. `cd rajeshdb && git pull && cd deploy && docker compose up -d --build`
3. Check `docker compose ps`, the dashboard, and `/ops/health`.
4. Rollback: `git checkout <previous tag>` and `docker compose up -d --build`.
There is **no format-migration tooling yet**: if a future version changes the on-disk format, the release notes must say how to
convert. Until then, never upgrade without a fresh backup you have verified.

## 11. Monitoring

* Public uptime check (free tools such as UptimeRobot or Better Stack): `https://console.example.com/api/health`.
* Detailed health for your own scripts, from an IP in `OPS_ALLOW_IPS` or through an SSH tunnel:
  `curl -H "Authorization: Bearer <operator token>" https://console.example.com/api/ops/health` (HTTP 503 = critical).
* Prometheus/Grafana scrape config: docs/OPERATIONS.md section 7 (point it at `127.0.0.1:8081` from a tunnel or a sidecar; the
  compose file does not publish 8081).
* Logs: `docker compose logs -f --since 1h platform` (JSON lines, never contain bodies or credentials; rotated by Docker: 5 x 20 MB).

## 12. Security checklist before real users

- [ ] SSH key login only; password login disabled; firewall allows only 22, 80, 443
- [ ] `.env` is not in Git; operator token is 32+ random bytes; `OPS_ALLOW_IPS` is your IP only
- [ ] Port 8081 is NOT published (`docker compose ps` shows no `0.0.0.0:8081`)
- [ ] HTTPS works and `/api/ops/...` returns 404 from outside
- [ ] `RAJESHDB_ALLOW_REGISTRATION=false` unless you want open sign-ups
- [ ] Off-site backup copy exists and a restore drill succeeded
- [ ] Disk budget alert configured; uptime monitor configured
- [ ] CI green; `cargo audit` / `npm audit` findings reviewed
- [ ] You told your users the limits (section 14) instead of promising "unlimited"

## 13. Monthly cost estimates (guidance, not a quote)

Prices below come from 2026 third-party price lists and provider announcements, exclude taxes (GST/VAT), and change often:
open the provider's own pricing page on the day you buy. Convert USD/EUR to rupees at the current rate.

| Level | What you run | Roughly per month |
|---|---|---|
| Development | your laptop | 0 |
| Free staging / demo | Oracle Always Free ARM VM (reported 2 OCPU / 12 GB), domain optional | 0, but may be throttled, reclaimed or unavailable; no SLA |
| First real users (a few projects, < ~5 GB data) | one 2 GB VPS (about USD 12 on DigitalOcean's list; EU alternatives vary), domain (about USD 1-2 amortized), off-site backup storage (a small object-storage bucket or a second cheap server: about USD 0-5) | about USD 13-20 |
| Growing (tens of GB, steady traffic) | 4 GB VPS (about USD 24), block volume at about USD 0.10 per GB, provider snapshots about USD 0.06 per GB, off-site storage | about USD 30-60 |
| Beyond one server | needs replication / high availability, which this project does not have (single node only) | not supported yet |

Free for development; **not free for something you keep running reliably**. Never promise unlimited storage, users or traffic:
the limits are the quota variables in `.env`, your disk, your RAM and your single server.

## 14. What this deployment does NOT give you

* Single node: if the server is down, the service is down (no replication, no automatic failover).
* No point-in-time recovery; backups are periodic snapshots.
* No format-migration tooling; upgrades rely on backups.
* Throttling and limits are per server process; there is no global WAF or DDoS protection (put the site behind a CDN/WAF provider if you need that).
* The whole stack is new and was never load-tested; no performance numbers exist (Phase 9).
