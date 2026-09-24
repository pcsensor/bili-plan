#!/usr/bin/env python3
"""Fail when dependency direction or the persisted cross-end contract drifts."""

import json
import re
import subprocess
import sys
from pathlib import Path
from rust_imports import crate_roots

ROOT = Path(__file__).resolve().parents[1]
errors: list[str] = []


def require(condition: bool, message: str) -> None:
    if not condition:
        errors.append(message)


metadata = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        text=True,
    )
)
members = {package["name"]: package for package in metadata["packages"]}
expected = {"planner-domain", "bili-planner", "bili-plan-server"}
require(expected <= members.keys(), f"workspace members must include {sorted(expected)}")
for name, package in members.items():
    local_dependencies = {
        dependency["name"] for dependency in package["dependencies"] if dependency.get("path")
    }
    if name == "planner-domain":
        require(not local_dependencies, "domain must not depend on an application crate")
        allowed = {"chrono", "serde", "serde_json"}
        direct = {dependency["name"] for dependency in package["dependencies"]}
        require(direct <= allowed, f"domain has an infrastructure dependency: {direct - allowed}")
    elif name in {"bili-planner", "bili-plan-server"}:
        require(
            local_dependencies == {"planner-domain"},
            f"{name} must depend only on planner-domain locally: {local_dependencies}",
        )

for path in (ROOT / "crates/domain/src").rglob("*.rs"):
    source = path.read_text()
    for forbidden in ("gpui", "rusqlite", "reqwest", "ureq", "axum", "std::fs", "std::net"):
        require(forbidden not in source, f"domain imports infrastructure token {forbidden}: {path}")

domain_edges = {
    "catalog": set(),
    "model": set(),
    "plan": {"catalog"},
    "schedule_recovery": {"model"},
    "source": set(),
    "study": {"model", "plan", "schedule_recovery", "source"},
}
for module, allowed in domain_edges.items():
    path = ROOT / "crates/domain/src" / f"{module}.rs"
    imports = crate_roots(path.read_text())
    require(imports <= allowed, f"domain dependency direction violated in {module}: {imports - allowed}")

desktop_adapter_edges = {
    "error": set(),
    "model": set(),
    "api": {"error", "model"},
    "parse": {"api", "error", "model"},
    "jellyfin": {"api", "error", "parse"},
    "fnos": {"api", "error", "parse"},
    "export": {"parse", "plan"},
}
for module, allowed in desktop_adapter_edges.items():
    paths = [(ROOT / "src" / f"{module}.rs")]
    paths.extend((ROOT / "src" / module).rglob("*.rs") if (ROOT / "src" / module).exists() else ())
    for path in paths:
        imports = crate_roots(path.read_text())
        require(imports <= allowed, f"desktop adapter depends upward in {path.relative_to(ROOT)}: {imports - allowed}")

server_edges = {
    "auth": set(),
    "ratelimit": set(),
    "models": set(),
    "card": {"models"},
    "feishu": set(),
    "store": {"models", "schedule_recovery"},
    "telegram": {"models", "store"},
    "scheduler": {"card", "feishu", "store", "telegram"},
}
for module, allowed in server_edges.items():
    source = (ROOT / "server/src" / f"{module}.rs").read_text()
    imports = crate_roots(source)
    require(imports <= allowed, f"server dependency direction violated in {module}: {imports - allowed}")

for path in list((ROOT / "src").rglob("*.rs")) + list((ROOT / "server/src").rglob("*.rs")):
    source = path.read_text()
    require("#[path =" not in source and "include!(" not in source, f"cross-file include: {path}")
    if "server/src" in str(path):
        require("bili_planner::" not in source, f"server depends on desktop: {path}")
    if path.name not in {"app.rs", "main.rs", "theme.rs", "assets.rs", "lib.rs"} and "src/app/" not in str(path):
        require("use gpui" not in source, f"UI import leaked into non-UI code: {path}")

model = (ROOT / "crates/domain/src/model.rs").read_text()
contract = json.loads((ROOT / "tools/schema_contract.json").read_text())
for name, expected_fields in contract.items():
    declaration = re.search(r"pub struct " + name + r"\s*\{(?P<body>.*?)\n\}", model, re.S)
    require(declaration is not None, f"missing persisted type: {name}")
    if declaration:
        actual_fields = dict(
            re.findall(r"^\s*pub\s+(\w+)\s*:\s*(.+),$", declaration.group("body"), re.M)
        )
        require(actual_fields == expected_fields, f"{name} contract changed: {actual_fields}")
    declarations = [
        path
        for base in (ROOT / "src", ROOT / "server/src", ROOT / "crates/domain/src")
        for path in base.rglob("*.rs")
        if re.search(r"\bpub struct " + name + r"\s*\{", path.read_text())
    ]
    require(len(declarations) == 1, f"{name} must have one definition: {declarations}")

for path, maximum in {
    "src/app.rs": 1000,
    "src/core.rs": 1000,
    "src/fnos.rs": 900,
    "crates/domain/src/study.rs": 1600,
    "server/src/store.rs": 1000,
}.items():
    actual = len((ROOT / path).read_text().splitlines())
    require(actual <= maximum, f"{path} grew to {actual} lines (budget {maximum}); split responsibilities")
for path in (ROOT / "src/app").rglob("*.rs"):
    actual = len(path.read_text().splitlines())
    require(actual <= 1200, f"{path.relative_to(ROOT)} grew to {actual} lines; split the feature")
for path in (ROOT / "src/fnos").rglob("*.rs"):
    actual = len(path.read_text().splitlines())
    require(actual <= 900, f"{path.relative_to(ROOT)} grew to {actual} lines; split the adapter")

cloud = (ROOT / "src/core/cloud.rs").read_text()
sync = cloud.split("pub fn sync_with_cloud(", 1)[-1]
require("save_config(" not in sync, "background cloud sync must not persist its snapshot")
for path in (
    ROOT / "src/core/cloud.rs",
    ROOT / "server/src/auth.rs",
    ROOT / "server/src/main.rs",
    ROOT / "server/.env.example",
):
    source = path.read_text()
    require(
        "ALLOW_LEGACY_TOKEN_TRANSPORT" not in source
        and "legacy_transport" not in source
        and "send_legacy_cloud" not in source
        and "?device_token=" not in source,
        f"insecure token transport returned: {path}",
    )
sync_payload = (ROOT / "server/src/models.rs").read_text().split("pub struct SyncPayload {", 1)[-1].split("\n}", 1)[0]
require("pub device_token:" not in sync_payload, "sync body must not carry the device credential")

if errors:
    for error in errors:
        print(f"architecture error: {error}", file=sys.stderr)
    sys.exit(1)
print("architecture and persisted contract checks passed")
