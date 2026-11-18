# Prettier browser assets

The interview editor formats JavaScript with Prettier 3.10.1 when the candidate
presses the toolbar's "Format JS" button or Shift+Alt+F. The version is pinned
here on its own: package.json's Prettier formats the repository and moves with
its dependency updates, and this copy changes only when someone bumps it here.

The standalone formatter is fetched here; the estree printer and the acorn
parser are fetched into the sibling prettier-plugins directory, because each
FETCH names one base URL. Acorn rather than Babel: it parses standard
ECMAScript only, which is what the JavaScript runner executes, it rejects
incomplete code instead of recovering from it, and it is half the size.

web/format.js imports the three files the first time a candidate formats, so
a candidate who never formats never downloads them. None of them evaluates a
string, so they run under the interview page's Content-Security-Policy as it
is.

THIRD-PARTY-NOTICES.md is Prettier's own list of what its bundles include,
acorn and acorn-jsx among them; it is fetched and pinned with the modules.

Run scripts/fetch-vendor.sh to download and scripts/verify-vendor.sh to verify.
The fetched files are ignored by Git. Updating Prettier requires updating both
FETCH URLs, both SHA256SUMS and the LICENSE together, and the version in the
repository's THIRD-PARTY-NOTICES.md.

Upstream: https://github.com/prettier/prettier
