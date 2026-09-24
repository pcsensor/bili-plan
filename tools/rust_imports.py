"""Small lexical helper for architecture checks of `crate::` paths."""

import re

CRATE_PATH = re.compile(r"\bcrate\s*::\s*")
ROOT = re.compile(r"\s*([A-Za-z_][A-Za-z_0-9]*)")


def crate_roots(source: str) -> set[str]:
    """Return roots from both `crate::foo` and `crate::{foo, bar::x}`.

    Nested use trees are scanned by brace depth so every top-level branch is
    checked. The architecture checker deliberately inspects all crate paths,
    including paths outside `use` declarations.
    """
    roots: set[str] = set()
    for path in CRATE_PATH.finditer(source):
        start = path.end()
        if source[start:start + 1] != "{":
            match = ROOT.match(source, start)
            if match and match.group(1) != "self":
                roots.add(match.group(1))
            continue

        depth = 1
        branch = start + 1
        for index in range(branch, len(source)):
            char = source[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            if (depth == 1 and char == ",") or depth == 0:
                match = ROOT.match(source, branch, index)
                if match and match.group(1) != "self":
                    roots.add(match.group(1))
                branch = index + 1
            if depth == 0:
                break
    return roots
