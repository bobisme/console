#!/usr/bin/env python3
"""Check that build-cart exposes the current console API and tool inventory."""

from __future__ import annotations

import re
import sys
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = Path(__file__).resolve().parents[3]
SKILL = SKILL_DIR / "SKILL.md"
REFERENCES = SKILL_DIR / "references"

# String literals that look like public CLI flags but are deliberately not
# accepted options. Keep each exclusion narrow and explain why it is safe.
IGNORED_AGENT_FLAGS = {
    "--bogus": "negative parser-test sentinel",
}


def fail(errors: list[str], message: str) -> None:
    errors.append(message)


def markdown_links(text: str) -> list[str]:
    return re.findall(r"\[[^]]+\]\(([^)]+\.md(?:#[^)]+)?)\)", text)


def contains_code_name(docs: str, name: str) -> bool:
    return f"`{name}`" in docs or f"`{name}(" in docs


def contains_cli_token(docs: str, token: str) -> bool:
    """Match one complete flag, never a prefix/suffix of another flag."""
    return re.search(
        rf"(?<![A-Za-z0-9-]){re.escape(token)}(?![A-Za-z0-9-])", docs
    ) is not None


def main() -> int:
    errors: list[str] = []
    skill_text = SKILL.read_text(encoding="utf-8")
    reference_files = sorted(REFERENCES.glob("*.md"))
    reference_texts = {
        path: path.read_text(encoding="utf-8") for path in reference_files
    }
    docs = "\n".join([skill_text, *reference_texts.values()])

    if len(skill_text.splitlines()) > 500:
        fail(errors, "SKILL.md exceeds the 500-line progressive-disclosure limit")

    linked_from_skill = {
        (SKILL_DIR / link.split("#", 1)[0]).resolve()
        for link in markdown_links(skill_text)
    }
    for path in reference_files:
        if path.resolve() not in linked_from_skill:
            fail(errors, f"reference is not linked directly from SKILL.md: {path.name}")

    for source, text in reference_texts.items():
        if len(text.splitlines()) > 100 and "## Contents" not in text:
            fail(errors, f"long reference has no Contents section: {source}")
        for link in markdown_links(text):
            target = link.split("#", 1)[0]
            if target and not (source.parent / target).exists():
                fail(errors, f"broken Markdown link in {source}: {link}")

    api_source = (REPO_ROOT / "crates/console-core/src/api.rs").read_text(
        encoding="utf-8"
    )
    lua_names = sorted(set(re.findall(r'g\.set\(\s*"([a-z_]+)"', api_source)))
    for name in lua_names:
        if not contains_code_name(docs, name):
            fail(errors, f"Lua API global is undocumented: {name}")

    agent_lib = (REPO_ROOT / "crates/console-agent/src/lib.rs").read_text(
        encoding="utf-8"
    )
    top_commands = sorted(
        command
        for command in set(re.findall(r'Some\("([a-z-]+)"', agent_lib))
        if not command.startswith("-")
    )
    for command in top_commands:
        if f"console-agent {command}" not in docs:
            fail(errors, f"top-level console-agent command is undocumented: {command}")

    agent_source = "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((REPO_ROOT / "crates/console-agent/src").rglob("*.rs"))
    )
    source_agent_flags = set(
        re.findall(r'"(--[a-z][a-z0-9-]*)"', agent_source)
    )
    for flag, reason in IGNORED_AGENT_FLAGS.items():
        if flag not in source_agent_flags:
            fail(errors, f"stale console-agent flag exclusion {flag}: {reason}")
    agent_flags = sorted(source_agent_flags - IGNORED_AGENT_FLAGS.keys())
    for flag in agent_flags:
        if not contains_cli_token(docs, flag):
            fail(errors, f"console-agent flag is undocumented: {flag}")
    for short_flag in ("-h", "-o"):
        if not contains_cli_token(docs, short_flag):
            fail(errors, f"console-agent short flag is undocumented: {short_flag}")

    for family in ("sprite", "map", "music"):
        module = (REPO_ROOT / f"crates/console-agent/src/{family}/mod.rs").read_text(
            encoding="utf-8"
        )
        inventory = re.search(
            r"pub const COMMANDS: &\[&str\] = &\[(.*?)\];", module, re.DOTALL
        )
        if inventory is None:
            fail(errors, f"cannot read {family} command inventory")
            continue
        for command in re.findall(r'"([a-z-]+)"', inventory.group(1)):
            phrase = f"console-agent {family} {command}"
            if phrase not in docs:
                fail(errors, f"console-agent leaf is undocumented: {family} {command}")

    required_edit_phrases = [
        "sprite edit <cart> shift",
        "sprite edit <cart> flip",
        "sprite edit <cart> rotate",
        "sprite edit <cart> copy",
        "sprite edit <cart> clear",
        "map edit <cart> copy",
        "map edit <cart> shift",
        "map edit <cart> fill",
        "map edit <cart> clear",
        "music edit <cart> transpose",
        "music edit <cart> copy",
        "music edit <cart> shift-rows",
        "music edit <cart> set-vol",
        "music edit <cart> set-inst",
        "music edit <cart> stretch",
    ]
    for phrase in required_edit_phrases:
        if phrase not in docs:
            fail(errors, f"write-command form is undocumented: {phrase}")

    rpc_source = (REPO_ROOT / "crates/console-agent/src/rpc.rs").read_text(
        encoding="utf-8"
    )
    rpc_methods = sorted(
        set(re.findall(r'^\s*"([a-z_]+)"\s*=>\s*m_', rpc_source, re.MULTILINE))
    )
    for method in rpc_methods:
        if not contains_code_name(docs, method):
            fail(errors, f"JSON-RPC method is undocumented: {method}")

    for flag in ("--out", "--output", "--engine", "--template", "--help"):
        if not contains_cli_token(docs, flag):
            fail(errors, f"console-pack flag is undocumented: {flag}")

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1

    print(
        "build-cart reference coverage: PASS "
        f"({len(lua_names)} Lua globals, {len(top_commands)} top-level commands, "
        f"{len(agent_flags)} console-agent flags, {len(rpc_methods)} RPC methods, "
        f"{len(reference_files)} references)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
