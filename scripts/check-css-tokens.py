#!/usr/bin/env python3
"""
Keep color literals in web/ CSS inside theme tokens.
A color literal (hex, rgb()/hsl()/oklch() and kin, or a named color) may only
be the value of a custom property declared directly in a :root, .practice-page
or .editor-stack rule, optionally qualified by [data-theme] or nested in a media
query. Mask gradients are exempt because a mask reads only alpha.
JavaScript (e.g. web/cube.js) and desktop/ui/start.css, which keeps its own
token set, are out of scope.
"""

import re
import sys
from pathlib import Path


THEME_SCOPE_RE = re.compile(
    r"(?:\[[^\]]*\]\s+)?"
    r"(?::root|\.practice-page|\.editor-stack)"
    r"(?:\[[^\]]*\]|:not\([^)]*\))*"
)


def is_theme_scope(scopes):
    rules = [s for s in scopes if not s.startswith("@")]
    return bool(rules) and all(
        THEME_SCOPE_RE.fullmatch(sel.strip()) for sel in rules[-1].split(",")
    )

IDENTIFIER_PROPS = {
    "font-family", "font", "animation-name", "animation",
    "grid-area", "grid-template-areas", "container-name"
}

def is_violation(prop_name: str, val: str, scopes: list[str]) -> bool:
    p_lower = prop_name.lower()

    if p_lower in ("mask", "-webkit-mask", "mask-image", "-webkit-mask-image"):
        return False

    has_color = False
    for color_match in COLOR_RE.finditer(val):
        color_str = color_match.group()
        if not color_str.startswith("#") and "(" not in color_str:
            if p_lower in IDENTIFIER_PROPS:
                continue
        has_color = True
        break

    if not has_color:
        return False
    if p_lower.startswith("--") and is_theme_scope(scopes):
        return False

    return True


ROOT = Path(__file__).resolve().parents[1]

STRIP_RE = re.compile(
    r"/\*.*?\*/|"
    r'"(?:\\.|[^"\\])*"|'
    r"'(?:\\.|[^'\\])*'|"
    r"url\([^)]*\)",
    re.DOTALL | re.IGNORECASE,
)

NAMED_COLORS = (
    "aliceblue|antiquewhite|aqua|aquamarine|azure|beige|bisque|black|blanchedalmond|blue|"
    "blueviolet|brown|burlywood|cadetblue|chartreuse|chocolate|coral|cornflowerblue|cornsilk|"
    "crimson|cyan|darkblue|darkcyan|darkgoldenrod|darkgray|darkgreen|darkgrey|darkkhaki|"
    "darkmagenta|darkolivegreen|darkorange|darkorchid|darkred|darksalmon|darkseagreen|"
    "darkslateblue|darkslategray|darkslategrey|darkturquoise|darkviolet|deeppink|deepskyblue|"
    "dimgray|dimgrey|dodgerblue|firebrick|floralwhite|forestgreen|fuchsia|gainsboro|"
    "ghostwhite|gold|goldenrod|gray|green|greenyellow|grey|honeydew|hotpink|indianred|indigo|"
    "ivory|khaki|lavender|lavenderblush|lawngreen|lemonchiffon|lightblue|lightcoral|lightcyan|"
    "lightgoldenrodyellow|lightgray|lightgreen|lightgrey|lightpink|lightsalmon|lightseagreen|"
    "lightskyblue|lightslategray|lightslategrey|lightsteelblue|lightyellow|lime|limegreen|"
    "linen|magenta|maroon|mediumaquamarine|mediumblue|mediumorchid|mediumpurple|"
    "mediumseagreen|mediumslateblue|mediumspringgreen|mediumturquoise|mediumvioletred|"
    "midnightblue|mintcream|mistyrose|moccasin|navajowhite|navy|oldlace|olive|olivedrab|"
    "orange|orangered|orchid|palegoldenrod|palegreen|paleturquoise|palevioletred|papayawhip|"
    "peachpuff|peru|pink|plum|powderblue|purple|rebeccapurple|red|rosybrown|royalblue|"
    "saddlebrown|salmon|sandybrown|seagreen|seashell|sienna|silver|skyblue|slateblue|"
    "slategray|slategrey|snow|springgreen|steelblue|tan|teal|thistle|tomato|turquoise|violet|"
    "wheat|white|whitesmoke|yellow|yellowgreen"
)

COLOR_RE = re.compile(
    r"(#[0-9a-fA-F]{3,8}\b|"
    r"\b(?:rgb|rgba|hsl|hsla|oklch|oklab|lab|lch|hwb|color)\([^)]+\)|"
    rf"(?<!-)\b(?:{NAMED_COLORS})\b(?!-))",
    re.IGNORECASE,
)

CHUNK_RE = re.compile(r"([^{};]*)([{};])")


def check_css_file(path: Path) -> int:
    text = STRIP_RE.sub(
        lambda m: "\n" * m.group().count("\n"), path.read_text(encoding="utf-8")
    )
    errors = 0
    scopes = []

    for match in CHUNK_RE.finditer(text):
        content, term = match.groups()

        if term == "{":
            scopes.append(content.strip())
        elif term in ("}", ";"):
            prop, sep, val = content.partition(":")
            if sep:
                prop_name = prop.strip()
                if is_violation(prop_name, val, scopes):
                    # 重新啟用 jserv 要求的 decl_start 計算方式，並透過括號換行來繞過 88 字元限制
                    decl_start = (
                        match.start() + len(content) - len(content.lstrip())
                    )
                    line_num = text.count("\n", 0, decl_start) + 1

                    display_path = (
                        path.relative_to(ROOT) if path.is_relative_to(ROOT) else path
                    )

                    print(
                        f"{display_path}:{line_num}: color literal in "
                        f"'{prop_name}', move it to a custom property in a theme block"
                    )
                    errors += 1

            if term == "}":
                if scopes:
                    scopes.pop()

    return errors


def main(root: Path = ROOT):
    css_files = (root / "web").rglob("*.css")
    total_errors = sum(check_css_file(path) for path in css_files)

    if total_errors > 0:
        print(f"Found {total_errors} color token violation(s).")
        sys.exit(1)


if __name__ == "__main__":
    main()