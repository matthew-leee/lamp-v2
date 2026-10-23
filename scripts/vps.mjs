#!/usr/bin/env node
/**
 * One-line Hetzner CX33 VPS operations & Git auto-deploy CLI (`OPS-01`, `DEPLOY-01`).
 *
 * Reads `HETZNER_VPS_PASSPHRASE` (and optional `HETZNER_VPS_HOST`, `HETZNER_VPS_KEY`)
 * from `.env` automatically so no inline SSH/passphrase boilerplate is ever needed.
 *
 * Usage:
 *   node scripts/vps.mjs deploy          # Push current Git HEAD to VPS & auto-rebuild containers
 *   node scripts/vps.mjs logs [service]  # Tail container logs (default: web api) without manual SSH
 *   node scripts/vps.mjs status          # Show container status, health & Cloudflare Tunnel URL
 *   node scripts/vps.mjs reset-db        # Reset orders/reservations to pristine 8 SKUs / 16 units
 *   node scripts/vps.mjs exec "<cmd>"    # Run an arbitrary shell command on the VPS
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const envPath = path.join(repoRoot, ".env");

function loadEnv(filePath) {
  const vars = {};
  if (!fs.existsSync(filePath)) return vars;
  const raw = fs.readFileSync(filePath, "utf8");
  for (const line of raw.split(/\r?\n/)) {
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/);
    if (m) {
      vars[m[1]] = m[2].trim().replace(/^["']|["']$/g, "");
    }
  }
  return vars;
}

const envVars = loadEnv(envPath);
const vpsHost = process.env.HETZNER_VPS_HOST || envVars.HETZNER_VPS_HOST || "root@89.167.15.159";
const vpsKey =
  process.env.HETZNER_VPS_KEY ||
  envVars.HETZNER_VPS_KEY ||
  path.join(os.homedir(), ".ssh", "hetznerUbuntu", "id_ed25519").replace(/\\/g, "/");
const passphrase = process.env.HETZNER_VPS_PASSPHRASE || envVars.HETZNER_VPS_PASSPHRASE || "";

function withAskPass(fn) {
  const askpass = path.join(os.tmpdir(), `vps_askpass_${process.pid}.bat`);
  fs.writeFileSync(askpass, "@echo off\r\necho %HETZNER_VPS_PASSPHRASE%\r\n", {
    mode: 0o700,
  });
  const childEnv = {
    ...process.env,
    HETZNER_VPS_PASSPHRASE: passphrase,
    SSH_ASKPASS: askpass,
    SSH_ASKPASS_REQUIRE: "force",
    DISPLAY: "1",
    GIT_SSH_COMMAND: `ssh -o StrictHostKeyChecking=accept-new -i "${vpsKey}"`,
  };
  try {
    return fn(childEnv);
  } finally {
    try {
      fs.unlinkSync(askpass);
    } catch {
      // ignore
    }
  }
}

function runSsh(remoteCmd, childEnv, timeout = 180000) {
  const res = spawnSync(
    "ssh",
    ["-o", "StrictHostKeyChecking=accept-new", "-i", vpsKey, vpsHost, remoteCmd],
    { env: childEnv, encoding: "utf8", stdio: "inherit", timeout },
  );
  if (res.status !== 0) {
    process.exit(res.status ?? 1);
  }
}

const [subcmd = "status", ...rest] = process.argv.slice(2);

withAskPass((childEnv) => {
  if (subcmd === "status") {
    runSsh(
      [
        "cd /opt/storefront-v2",
        "docker compose ps",
        "echo '--- CLOUDFLARE TUNNEL ---'",
        "docker compose logs cloudflared 2>&1 | grep -E 'trycloudflare\\.com' | tail -n 2 || true",
        "echo '--- HEALTH ---'",
        "curl -sS https://89-167-15-159.sslip.io/health && echo ''",
      ].join(" && "),
      childEnv,
      30000,
    );
  } else if (subcmd === "logs") {
    const services = rest.length > 0 ? rest.join(" ") : "web api";
    runSsh(
      `cd /opt/storefront-v2 && docker compose logs --tail=80 ${services}`,
      childEnv,
      30000,
    );
  } else if (subcmd === "reset-db") {
    const sql = [
      "TRUNCATE TABLE payment_events, order_items, orders RESTART IDENTITY CASCADE;",
      "DELETE FROM products WHERE id > 8;",
      "UPDATE products SET stock_total = 2, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'panthella-160-v3-beige';",
      "UPDATE products SET stock_total = 5, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'panthella-160-v3-white-opal';",
      "UPDATE products SET stock_total = 2, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'panthella-160-v3-opal-beige-chrome';",
      "UPDATE products SET stock_total = 1, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'panthella-160-v3-opal-pale-blue';",
      "UPDATE products SET stock_total = 1, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'panthella-160-v3-opal-pale-rose';",
      "UPDATE products SET stock_total = 3, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'ph-2-1-table-chrome';",
      "UPDATE products SET stock_total = 1, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'ph-3-2-table-chrome';",
      "UPDATE products SET stock_total = 1, stock_reserved = 0, stock_quarantined = 0, active = TRUE WHERE slug = 'ph-1-1-piano-table-limited';",
      "SELECT COUNT(*) AS orders_count FROM orders;",
      "SELECT SUM(stock_total) AS total_units, SUM(stock_reserved) AS reserved_units FROM products WHERE active;",
    ].join("\n");
    runSsh(
      `docker exec -i storefront-v2-db-1 psql -U storefront -d storefront << 'EOF'\n${sql}\nEOF`,
      childEnv,
      30000,
    );
  } else if (subcmd === "deploy") {
    // Ensure bare repo + post-receive auto-deploy hook exist on VPS, then git push HEAD
    const hookScript = [
      "#!/usr/bin/env sh",
      "set -eu",
      "echo '[vps-hook] Checking out latest commit into /opt/storefront-v2...'",
      "git --work-tree=/opt/storefront-v2 --git-dir=/opt/storefront-v2.git checkout -f v2-clean-room",
      "cd /opt/storefront-v2",
      // 1. Database FIRST: the migration below needs it running, and the api
      //    healthcheck gates the rest of the stack anyway. Wait for Postgres
      //    to actually accept queries (`pg_isready`) — `up -d db` returning
      //    does NOT mean the server is ready, and a migration dropped on a
      //    booting server would fail the deploy.
      "echo '[vps-hook] Starting the database...'",
      "docker compose up -d db",
      "echo '[vps-hook] Waiting for Postgres readiness...'",
      "until docker compose exec -T db sh -c 'pg_isready -U \"${POSTGRES_USER:-storefront}\" -d \"${POSTGRES_DB:-storefront}\"' >/dev/null 2>&1; do sleep 1; done",
      // 2. Idempotent schema hardening (G01/G02/G17 — 0008 uses IF NOT EXISTS /
      //    additive columns only): converge an existing production database on
      //    the new schema BEFORE any code that requires it starts.
      "echo '[vps-hook] Applying idempotent migration 0008 (payment / email / session hardening)...'",
      "docker compose exec -T db sh -c 'psql -U \"${POSTGRES_USER:-storefront}\" -d \"${POSTGRES_DB:-storefront}\" -v ON_ERROR_STOP=1' < api/migrations/0008_payment_email_session_hardening.sql",
      // 3. Preserve operator photo uploads (G18): pull whatever the existing
      //    web container has at /app/public/uploads (container filesystem on a
      //    pre-volume deployment, the volume itself otherwise) into a staging
      //    dir BEFORE the rebuild replaces that container, then seed the
      //    uploads_data volume through a throwaway web container mounting it.
      //    Skipped entirely when there is nothing to migrate (first deploy).
      //    A FAILED copy FAILS the deploy (no `|| true`): losing operator
      //    photos silently is worse than a red hook. The seeded files are
      //    re-owned to the web container user (1000:1000) so it can still
      //    write NEW uploads into the volume afterwards.
      "echo '[vps-hook] Preserving operator uploads into the uploads_data volume...'",
      "mkdir -p /tmp/storefront_v2_uploads_staging",
      "if docker container inspect storefront-v2-web-1 >/dev/null 2>&1; then",
      "  docker cp storefront-v2-web-1:/app/public/uploads/. /tmp/storefront_v2_uploads_staging/",
      "fi",
      "if [ -n \"$(ls -A /tmp/storefront_v2_uploads_staging 2>/dev/null)\" ]; then",
      "  docker compose run --rm --no-deps -T --user root --entrypoint sh -v /tmp/storefront_v2_uploads_staging:/from:ro web -c 'cp -a /from/. /app/public/uploads/ && chown -R 1000:1000 /app/public/uploads'",
      "fi",
      "rm -rf /tmp/storefront_v2_uploads_staging",
      // 4. Only now rebuild + restart the full stack (web now serves uploads
      //    from the volume via /api/uploads/{filename}). Caddy bind-mounts
      //    ops/Caddyfile, so it must be RESTARTED to pick up routing changes
      //    — `up -d --build` alone leaves the old config loaded.
      "echo '[vps-hook] Building and restarting Docker Compose services...'",
      "docker compose up -d --build",
      "docker compose restart caddy",
      "docker compose ps",
      "echo '[vps-hook] Deployment complete!'",
    ].join("\n");

    runSsh(
      `mkdir -p /opt/storefront-v2.git && cd /opt/storefront-v2.git && git init --bare -q && cat > hooks/post-receive << 'EOF'\n${hookScript}\nEOF\nchmod +x hooks/post-receive`,
      childEnv,
      30000,
    );

    const pushRes = spawnSync(
      "git",
      ["push", `${vpsHost}:/opt/storefront-v2.git`, "HEAD:refs/heads/v2-clean-room", "--force"],
      { cwd: repoRoot, env: childEnv, stdio: "inherit", timeout: 300000 },
    );
    if (pushRes.status !== 0) {
      process.exit(pushRes.status ?? 1);
    }
  } else if (subcmd === "exec") {
    const cmd = rest.join(" ");
    if (!cmd) {
      console.error("Usage: node scripts/vps.mjs exec \"<command>\"");
      process.exit(1);
    }
    runSsh(cmd, childEnv, 120000);
  } else {
    console.error(
      "Unknown command. Use: deploy | logs [service] | status | reset-db | exec \"<cmd>\"",
    );
    process.exit(1);
  }
});
