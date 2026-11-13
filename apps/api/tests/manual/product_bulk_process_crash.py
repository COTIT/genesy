#!/usr/bin/env python3
"""Opt-in local crash proof. Own disposable DB/container only; never alters a shared API.

Run only after other load tests finish. Requires existing local synthetic Postgres,
cached runtime image, Linux API binary, Docker, and psql. Uses no third-party Python packages.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time
import urllib.error
import urllib.request


IMAGE = "rust:1.99.0-slim-trixie@sha256:2752b332db73fdbb7dc576f06c82ed1f312005784ef913d7e04a28f5f55dc581"
GATE = 723416831


def utc():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def wait_for(label, callback, timeout=30, interval=0.1):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = callback()
        if value:
            return value
        time.sleep(interval)
    raise AssertionError(f"Timed out waiting for {label}")


class Harness:
    def __init__(self, args):
        self.args = args
        self.name = "bulk-crash-" + str(int(time.time()))
        self.database = self.name.replace("-", "_")
        assert re.fullmatch(r"bulk_crash_[0-9]+", self.database)
        self.out = args.output.resolve()
        self.out.mkdir(parents=True, exist_ok=False)
        self.report = {"started_at": utc(), "database": self.database, "container": self.name}
        self.docker = ["docker", "--host", args.docker_socket, "--config", str(args.docker_config)]
        self.db_env = dict(os.environ, PGPASSWORD="bulk_local_tests")
        self.token = None
        self.workspace = None
        self.run_id = None
        self.gate = None
        self.created_database = False
        self.created_container = False
        self.server = "http://127.0.0.1:4181"
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def cmd(self, command, **kwargs):
        return subprocess.run(command, text=True, check=True, capture_output=True, **kwargs).stdout.strip()

    def sql(self, query, database=None):
        return self.cmd(["psql", "-XAt", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-p", "54389",
                         "-U", "genesy_bulk", "-d", database or self.database, "-c", query], env=self.db_env)

    def sql_json(self, query):
        return json.loads(self.sql(query))

    def api(self, method, path, body=None):
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        payload = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.server + path, data=payload, headers=headers, method=method)
        try:
            with self.http.open(request, timeout=65) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise AssertionError(f"{method} {path}: {error.code} {error.read().decode()}") from error

    def healthy(self):
        try:
            with self.http.open(self.server + "/healthz", timeout=2) as response:
                return response.status == 200
        except (OSError, urllib.error.URLError):
            return False

    def run_path(self, suffix=""):
        return f"/v1/workspace/{self.workspace}/product-bulk-runs/{self.run_id}{suffix}"

    def env(self):
        return {
            "DATABASE_URL": f"postgres://genesy_bulk:bulk_local_tests@host.docker.internal:54389/{self.database}?application_name=bulk-crash-api",
            "DATABASE_MAX_CONNECTIONS": "4", "DATABASE_JOB_CONNECTIONS": "1",
            "BIND_ADDR": "0.0.0.0:8080", "FRONTEND_URL": self.server, "API_PUBLIC_URL": self.server,
            "SMTP_URL": "smtp://127.0.0.1:1025", "SMTP_ALLOW_INSECURE": "true",
            "MAIL_FROM": "crash@example.test", "SIGNUP_ENABLED": "true", "PRODUCT_BULK_ENABLED": "true",
            "SKIP_BOOT_MIGRATIONS": "true", "RATE_LIMIT_BURST": "1000", "RUST_LOG": "info",
        }

    def runtime(self):
        return ["--user", "65532:65532", "--read-only", "--memory", "512m", "--memory-swap", "512m",
                "--cpus", "1", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777", "--workdir", "/tmp",
                "--mount", f"type=bind,src={self.args.binary.resolve()},dst=/qa/genesy-api,readonly",
                "--entrypoint", "/qa/genesy-api"]

    def capture_memory(self):
        try:
            peak = int(self.cmd(self.docker + ["exec", self.name, "cat", "/sys/fs/cgroup/memory.peak"]))
            self.report["peak_container_bytes"] = max(peak, self.report.get("peak_container_bytes", 0))
        except (subprocess.CalledProcessError, ValueError):
            self.report["memory_peak_unavailable"] = True

    def start(self):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 4181))
        self.report["binary"] = str(self.args.binary.resolve())
        self.report["binary_sha256"] = hashlib.sha256(self.args.binary.read_bytes()).hexdigest()
        self.report["image"] = IMAGE
        self.sql(f'CREATE DATABASE "{self.database}"', "genesy_bulk")
        self.created_database = True
        # No Cargo work and no migration against an existing DB.
        env = self.env()
        migrated = self.cmd(self.docker + ["run", "--pull", "never", "--rm"] + self.runtime() +
                            ["--env", "DATABASE_URL=" + env["DATABASE_URL"], IMAGE, "migrate"])
        self.report["migration"] = migrated
        args = self.docker + ["run", "--pull", "never", "--detach", "--name", self.name, "-p", "127.0.0.1:4181:8080"] + self.runtime()
        for key, value in env.items():
            args += ["--env", key + "=" + value]
        self.report["container_id"] = self.cmd(args + [IMAGE])
        self.created_container = True
        wait_for("isolated API boot", self.healthy, 60, 0.25)
        settings = json.loads(self.cmd(self.docker + ["inspect", "--format",
            '{"memory":{{.HostConfig.Memory}},"user":{{json .Config.User}},"readonly":{{.HostConfig.ReadonlyRootfs}}}', self.name]))
        assert settings == {"memory": 536870912, "user": "65532:65532", "readonly": True}, settings
        self.report["runtime_limits"] = settings

    def setup(self):
        session = self.api("POST", "/v1/create-workspace", {"first_name": "Crash", "last_name": "Proof",
            "email": "crash@example.test", "password": "Str0ng!pass", "workspace_name": self.name})["data"]
        self.token = session["access_token"]
        self.workspace = session["workspace_ids"][0]
        base = f"/v1/workspace/{self.workspace}"
        self.api("POST", base + "/warehouses", {"name": "Crash store", "is_store": True})
        control = self.api("POST", base + "/products", {"name": "Native control", "price": "1"})["data"]["product_id"]
        self.report["native_control"] = self.sql_json(f"""SELECT json_build_object(
            'variants',(SELECT count(*) FROM product_variants WHERE product_id={literal(control)}),
            'history',(SELECT count(*) FROM product_price_histories WHERE product_id={literal(control)}),
            'stocks',(SELECT count(*) FROM stocks WHERE product_id={literal(control)}))""")
        assert all(v > 0 for v in self.report["native_control"].values())
        # Queue native event effects but never dial another service: credentials are absent.
        self.sql(f"""INSERT INTO webhook_endpoints(workspace_id,url,events,secret_sealed,secret_key_id)
            VALUES({literal(self.workspace)},'https://127.0.0.1:1',ARRAY['product.changed'],decode('00','hex'),'test-only')""")
        run = self.api("POST", base + "/product-bulk-runs", {"mode": "create", "name": "Actual process crash"})["data"]
        self.run_id = run["id"]
        run = self.api("POST", self.run_path("/stage"), {"revision": run["revision"], "rows": [
            {"operation": "create", "values": {"name": f"Crash {i:02}", "price": "1"}} for i in range(60)]})["data"]
        self.revision = run["revision"]
        wait_for("60 staged rows ready", lambda: self.api("GET", self.run_path())["data"]["counts"]["ready"] == 60, 60, 0.25)
        review = self.api("POST", self.run_path("/reviews"), {"revision": self.revision, "scope": "write"})["data"]
        self.review_id = review["id"]
        self.report.update(workspace_id=self.workspace, run_id=self.run_id, review_id=self.review_id)

    def install_gate(self):
        control = self.report["native_control"]
        self.sql(f"""CREATE SEQUENCE crash_gate_seen;
            CREATE FUNCTION crash_gate() RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE first_id uuid;
            BEGIN
              IF NEW.name='Crash 01' THEN
                SELECT id INTO first_id FROM products WHERE name='Crash 00';
                IF first_id IS NULL
                   OR (SELECT count(*) FROM product_variants WHERE product_id=first_id)<>{control['variants']}
                   OR (SELECT count(*) FROM product_price_histories WHERE product_id=first_id)<>{control['history']}
                   OR (SELECT count(*) FROM stocks WHERE product_id=first_id)<>{control['stocks']}
                   OR (SELECT count(*) FROM webhook_events WHERE payload->>'product_id'=first_id::text AND payload->>'action'='created')<>1
                   OR (SELECT count(*) FROM product_bulk_rows WHERE run_id={literal(self.run_id)} AND values->>'name'='Crash 00' AND status='applied' AND receipt->>'product_id'=first_id::text)<>1
                THEN RAISE EXCEPTION 'first-row native effects or receipt missing inside chunk'; END IF;
                PERFORM nextval('crash_gate_seen');
                PERFORM pg_advisory_xact_lock({GATE});
              END IF;
              RETURN NEW;
            END $$;
            CREATE TRIGGER crash_gate BEFORE INSERT ON products FOR EACH ROW EXECUTE FUNCTION crash_gate();""")
        env = dict(self.db_env, PGAPPNAME=self.name + "-gate")
        self.gate = subprocess.Popen(["psql", "-XAt", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-p", "54389",
            "-U", "genesy_bulk", "-d", self.database], env=env, stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
        self.gate.stdin.write(f"SELECT pg_advisory_lock({GATE});\n")
        self.gate.stdin.flush()
        wait_for("owned advisory gate", lambda: self.sql(f"SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE a.datname=current_database() AND a.application_name={literal(self.name+'-gate')} AND l.locktype='advisory' AND l.granted") == "1")

    def counts(self):
        return self.sql_json(f"""WITH p AS (SELECT id FROM products WHERE name LIKE 'Crash %')
            SELECT json_build_object('products',(SELECT count(*) FROM p),
              'variants',(SELECT count(*) FROM product_variants WHERE product_id IN(SELECT id FROM p)),
              'history',(SELECT count(*) FROM product_price_histories WHERE product_id IN(SELECT id FROM p)),
              'stocks',(SELECT count(*) FROM stocks WHERE product_id IN(SELECT id FROM p)),
              'events',(SELECT count(*) FROM webhook_events WHERE payload->>'product_id' IN(SELECT id::text FROM p)),
              'row_receipts',(SELECT count(*) FROM product_bulk_rows WHERE run_id={literal(self.run_id)} AND receipt IS NOT NULL),
              'review_receipts',(SELECT count(*) FROM product_bulk_review_rows WHERE review_id={literal(self.review_id)} AND receipt IS NOT NULL))""")

    def crash(self):
        self.api("POST", self.run_path("/approve"), {"revision": self.revision, "review_id": self.review_id})
        wait_for("second product blocked after first native effects", lambda: self.sql("SELECT is_called FROM crash_gate_seen") == "t", 30, 0.05)
        self.report["midrow_proven_at"] = utc()
        self.report["before_crash_visible"] = self.counts()
        assert all(v == 0 for v in self.report["before_crash_visible"].values())
        job = self.sql_json(f"SELECT row_to_json(j) FROM (SELECT id,status,locked_by,locked_until,progress_done,attempts FROM jobs WHERE payload->>'run_id'={literal(self.run_id)} AND payload->>'phase'='apply') j")
        assert job["status"] == "running" and job["progress_done"] == 0, job
        self.report["crashed_job"] = job
        self.capture_memory()
        self.cmd(self.docker + ["kill", "--signal", "KILL", self.name])
        self.report["killed_at"] = utc()
        status = json.loads(self.cmd(self.docker + ["inspect", "--format", "{{json .State}}", self.name]))
        assert status["ExitCode"] == 137 and not status["Running"] and not status["OOMKilled"], status
        self.report["crash_exit"] = {k: status[k] for k in ("ExitCode", "Running", "OOMKilled")}
        self.release_gate()
        wait_for("dead API transactions rolled back", lambda: self.sql("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND application_name='bulk-crash-api'") == "0", 15)
        self.report["after_crash_visible"] = self.counts()
        assert all(v == 0 for v in self.report["after_crash_visible"].values())
        progress = self.sql(f"SELECT progress_done FROM jobs WHERE id={literal(job['id'])}")
        assert progress == "0", progress
        self.sql("DROP TRIGGER crash_gate ON products; DROP FUNCTION crash_gate(); DROP SEQUENCE crash_gate_seen;")

    def release_gate(self):
        if self.gate:
            gate = self.gate
            self.gate = None
            try:
                if gate.poll() is None:
                    gate.stdin.write(f"SELECT pg_advisory_unlock({GATE});\n\\q\n")
                    gate.stdin.flush()
                    gate.stdin.close()
                gate.wait(timeout=10)
            finally:
                if gate.poll() is None:
                    gate.kill()
                    gate.wait(timeout=10)

    def recover(self):
        self.cmd(self.docker + ["start", self.name])
        self.report["restarted_at"] = utc()
        wait_for("restarted API", self.healthy, 60, 0.25)
        self.report["lease_recovery"] = "Natural 60-second rail lease and 30-second scheduler; no SQL lease edits."
        wait_for("60 exactly-once outcomes after natural lease recovery", lambda: self.api("GET", self.run_path())["data"]["state"] == "completed", 180, 1)
        self.report["completed_at"] = utc()
        assert dt.datetime.fromisoformat(self.report["completed_at"]) >= dt.datetime.fromisoformat(self.report["crashed_job"]["locked_until"])
        final = self.counts()
        control = self.report["native_control"]
        expected = {"products": 60, "variants": 60*control["variants"], "history": 60*control["history"],
                    "stocks": 60*control["stocks"], "events": 60, "row_receipts": 60, "review_receipts": 60}
        assert final == expected, (final, expected)
        self.report["final_counts"] = final
        distinct = self.sql_json(f"""SELECT json_build_object(
            'names',(SELECT count(DISTINCT name) FROM products WHERE name LIKE 'Crash %'),
            'numbers',(SELECT count(DISTINCT product_number) FROM products WHERE name LIKE 'Crash %'),
            'receipt_products',(SELECT count(DISTINCT receipt->>'product_id') FROM product_bulk_rows WHERE run_id={literal(self.run_id)}),
            'completed_receipts',(SELECT count(*) FROM product_bulk_rows WHERE run_id={literal(self.run_id)} AND status='applied' AND receipt->>'status'='completed'),
            'bad_receipt_links',(SELECT count(*) FROM product_bulk_rows r WHERE run_id={literal(self.run_id)} AND NOT EXISTS(SELECT 1 FROM products p WHERE p.id::text=r.receipt->>'product_id' AND p.name=r.values->>'name')),
            'single_history_groups',(SELECT count(*) FROM (SELECT product_id,product_variant_id,field FROM product_price_histories WHERE product_id IN(SELECT id FROM products WHERE name LIKE 'Crash %') GROUP BY product_id,product_variant_id,field HAVING count(*)<>1) x),
            'single_event_groups',(SELECT count(*) FROM (SELECT payload->>'product_id' FROM webhook_events GROUP BY payload->>'product_id' HAVING count(*)<>1) x))""")
        assert distinct == {"names":60,"numbers":60,"receipt_products":60,"completed_receipts":60,"bad_receipt_links":0,"single_history_groups":0,"single_event_groups":0}, distinct
        self.report["exact_once"] = distinct
        self.capture_memory()
        self.report["passed"] = True

    def cleanup(self):
        errors = []
        try:
            self.release_gate()
        except Exception as error:
            errors.append("gate: " + str(error))
        if self.created_container:
            logs = subprocess.run(self.docker + ["logs", self.name], text=True, capture_output=True)
            (self.out / "api.log").write_text(logs.stdout + logs.stderr)
            result = subprocess.run(self.docker + ["rm", "--force", self.name], text=True, capture_output=True)
            if result.returncode:
                errors.append("container: " + result.stderr)
        if self.created_database:
            try:
                self.sql(f'DROP DATABASE "{self.database}" WITH (FORCE)', "genesy_bulk")
            except Exception as error:
                errors.append("database: " + str(error))
        self.report["cleanup_errors"] = errors
        self.report["finished_at"] = utc()
        (self.out / "report.json").write_text(json.dumps(self.report, indent=2) + "\n")
        if errors:
            raise AssertionError(errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--docker-config", required=True, type=Path)
    parser.add_argument("--docker-socket", default="unix:///Users/cotit/.docker/run/docker.sock")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    harness = Harness(args)
    try:
        harness.start()
        harness.setup()
        harness.install_gate()
        harness.crash()
        harness.recover()
    except Exception as error:
        harness.report["failure"] = str(error)
        raise
    finally:
        harness.cleanup()
    print(json.dumps({k: harness.report.get(k) for k in ("passed", "binary_sha256", "final_counts", "peak_container_bytes", "cleanup_errors")}, indent=2))


if __name__ == "__main__":
    main()
