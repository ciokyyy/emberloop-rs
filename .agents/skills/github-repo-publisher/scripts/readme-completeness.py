#!/usr/bin/env python3
"""Lightweight README completeness check for github-repo-publisher."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REQUIRED_SECTIONS = [
    ("what this solves", ("what this solves", "problem", "purpose")),
    ("why it matters", ("why it matters", "value", "impact")),
    ("who it is for", ("who it is for", "who uses it", "audience")),
    ("how it works", ("how it works", "architecture", "technical")),
    ("quick start", ("quick start", "getting started", "local development")),
    ("commands", ("commands", "scripts")),
    ("configuration", ("configuration", "environment")),
    ("testing", ("testing", "testing and verification")),
    ("deployment/release", ("deployment", "release")),
    ("troubleshooting", ("troubleshooting", "known issues")),
    ("support", ("support", "help", "ownership", "operational notes")),
    ("security", ("security", "access and data")),
    ("status", ("status", "maintenance status", "roadmap")),
]

PLACEHOLDER_PATTERNS = [
    r"\bTODO\b",
    r"\bFIXME\b",
    r"lorem ipsum",
    r"PROJECT_NAME",
    r"SHORT_DESCRIPTION",
    r"OWNER_OR_",
    r"Example symptom",
    r"Example cause",
    r"Example fix",
    r"your[-_ ]?(api[-_ ]?)?key",
]

BADGE_IMAGE_RE = re.compile(r"!\[[^\]]*\]\(([^)]+)\)")
SHIELDS_STYLE_RE = re.compile(r"[?&]style=([^&)\s]+)")
UNSAFE_BADGE_TOKEN_RE = re.compile(r"[?&](token|api[_-]?key|access[_-]?token)=", re.IGNORECASE)
FAKE_STATUS_BADGE_RE = re.compile(
    r"https://img\.shields\.io/badge/[^)\s]*(passing|secure|production--ready|production_ready|100%25|100)",
    re.IGNORECASE,
)


def headings(markdown: str) -> list[str]:
    found: list[str] = []
    for line in markdown.splitlines():
        match = re.match(r"^#{1,6}\s+(.+?)\s*$", line)
        if match:
            found.append(match.group(1).strip().lower())
    return found


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: readme-completeness.py path/to/README.md", file=sys.stderr)
        return 2

    path = Path(sys.argv[1])
    if not path.is_file():
        print(f"README not found: {path}", file=sys.stderr)
        return 2

    text = path.read_text(encoding="utf-8")
    found_headings = headings(text)
    lower = "\n".join(found_headings)

    missing = []
    for label, aliases in REQUIRED_SECTIONS:
        if not any(alias in lower for alias in aliases):
            missing.append(label)

    placeholders = []
    for pattern in PLACEHOLDER_PATTERNS:
        if re.search(pattern, text, flags=re.IGNORECASE):
            placeholders.append(pattern)

    badge_urls = BADGE_IMAGE_RE.findall(text)
    badge_issues: list[str] = []
    badge_warnings: list[str] = []
    shields_styles: set[str] = set()
    for url in badge_urls:
        if "img.shields.io" in url:
            match = SHIELDS_STYLE_RE.search(url)
            if match:
                shields_styles.add(match.group(1))
        if UNSAFE_BADGE_TOKEN_RE.search(url):
            badge_issues.append(f"badge URL contains a token-like query parameter: {url}")
        if FAKE_STATUS_BADGE_RE.search(url):
            badge_issues.append(f"static badge appears to claim a live health/status signal: {url}")

    if len(badge_urls) > 5:
        badge_warnings.append(f"{len(badge_urls)} badges found; keep the default badge budget to 0-5 unless the repo profile justifies more")
    if len(shields_styles) > 1:
        badge_warnings.append(f"mixed Shields.io styles found: {', '.join(sorted(shields_styles))}")

    command_blocks = len(re.findall(r"```(?:bash|sh|shell|zsh)\b", text, flags=re.IGNORECASE))
    score = max(0, 100 - len(missing) * 6 - len(placeholders) * 8 - len(badge_issues) * 10)

    print(f"README completeness score: {score}/100")
    print(f"Command blocks: {command_blocks}")
    if missing:
        print("Missing sections:")
        for item in missing:
            print(f"- {item}")
    if placeholders:
        print("Placeholder-like text found:")
        for item in placeholders:
            print(f"- {item}")
    if badge_urls:
        print(f"Badges found: {len(badge_urls)}")
    if badge_warnings:
        print("Badge warnings:")
        for item in badge_warnings:
            print(f"- {item}")
    if badge_issues:
        print("Badge issues:")
        for item in badge_issues:
            print(f"- {item}")

    if score < 85 or placeholders or badge_issues:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
