#!/usr/bin/env python3
"""Scan the public repository tree for forbidden secrets, personal domains, or host IPs."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

FORBIDDEN_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("private_host_ip", re.compile(r"(?:141\.98\.196\.116|61\.124\.1\.9|23\.185\.168\.200|23\.169\.184\.147|180\.76\.247\.127|81\.69\.163\.191|149\.104\.6\.207)")),
    ("personal_domain", re.compile(r"(?:yizhimeng\.uk|buziyoudelihua\.com|youye\.edu\.rs)", re.IGNORECASE)),
    ("cloudflare_token", re.compile(r"cfat_[A-Za-z0-9]{10,}")),
    ("openai_or_deepseek_key", re.compile(r"sk-[A-Za-z0-9]{16,}")),
    ("personal_gmail", re.compile(r"[A-Za-z0-9._%+-]+@gmail\.com", re.IGNORECASE)),
]

SKIP_DIRS = {".git", "node_modules", "target", "dist", ".wrangler", "coverage"}


def iter_candidate_files() -> list[Path]:
    result = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode == 0 and result.stdout.strip():
        files = [ROOT / line for line in result.stdout.splitlines() if line.strip()]
        return [p for p in files if p.is_file() and p != Path(__file__).resolve()]

    collected: list[Path] = []
    for path in ROOT.rglob("*"):
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.is_file() and path != Path(__file__).resolve():
            collected.append(path)
    return collected


def main() -> int:
    findings: list[str] = []
    for path in iter_candidate_files():
        rel = path.relative_to(ROOT)
        if path.name.endswith(".secret") or path.name == ".env":
            findings.append(f"{rel}: forbidden secret filename")
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for lineno, line in enumerate(content.splitlines(), start=1):
            for label, pattern in FORBIDDEN_PATTERNS:
                if pattern.search(line):
                    findings.append(f"{rel}:{lineno}: matched {label}")

    if findings:
        print("FAIL: potential secret/PII leakage detected:")
        for item in findings:
            print(f"  - {item}")
        return 1

    print("PASS: 0 findings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
