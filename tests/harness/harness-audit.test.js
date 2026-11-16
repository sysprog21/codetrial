import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFile, execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { availableParallelism, tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import {
  generateHarness,
  parseCompilerResults,
} from "../../web/compiler-explorer.js";
import {
  loadJudges,
  prelude,
  stubCandidate,
} from "../../scripts/dump-harness.mjs";

// The C and C++ harnesses are helpers inside a template string, so nothing
// reads them the way it reads a source file: a C helper with a 4096-slot stack
// queue went unseen until an input of 5000 nodes crashed it. These checks treat
// each prelude and every judge's generated program as the code they are. The
// candidate is a stub that returns zero, which exercises the helpers that
// build inputs and look nodes up, and leaves the serializers on an empty
// result; a test that needs a real answer still goes through runJudge in
// tests/browser/compiler-explorer.test.js.
//
// It is its own gate and not a browser test: the browser lane is sharded by
// file size, which would put this wherever the sizes happen to fall, and the
// portable lane that runs it finishes well before the slowest browser shard.

const run = promisify(execFile);
const available = (command) =>
  spawnSync(command, ["--version"]).status === 0
    ? false
    : `${command} is not available`;

/// Runs `body` with a scratch directory and removes it after.
async function withTempDir(prefix, body) {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    return await body(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// Leak detection on, and a leak reported without changing the exit status, so
// a run's own failures are never confused with the builders' leaks.
const LEAK_ENV = {
  ASAN_OPTIONS: "detect_leaks=1",
  LSAN_OPTIONS: "exitcode=0",
};

// Whether `compiler` builds with both sanitizers and LeakSanitizer reports a
// leak. A program that leaks on purpose is the probe, since a runtime where
// leak detection is unavailable, or stops at start-up where ptrace is refused
// as in some containers, would pass a program that leaks nothing and then audit
// without a leak check. Each language is probed with its own compiler, so a
// host with one of them still audits that one.
function sanitizes(compiler, extension) {
  const dir = mkdtempSync(join(tmpdir(), "codetrial-audit-probe-"));
  try {
    writeFileSync(
      join(dir, `p.${extension}`),
      "#include <stdlib.h>\nint main(void) { void* volatile p = malloc(8); p = 0; return 0; }\n",
    );
    execFileSync(
      compiler,
      [
        "-fsanitize=address,undefined",
        join(dir, `p.${extension}`),
        "-o",
        join(dir, "p"),
      ],
      { stdio: "ignore" },
    );
    const probe = spawnSync(join(dir, "p"), [], {
      encoding: "utf8",
      env: { ...process.env, ...LEAK_ENV },
    });
    return probe.status === 0 && /leak of/.test(probe.stderr);
  } catch {
    return false;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
/// Why `compiler` cannot audit a harness, or false when it can.
const unusable = (compiler, extension) =>
  available(compiler) ||
  (sanitizes(compiler, extension)
    ? false
    : `${compiler} cannot run AddressSanitizer and report a leak`);

// What differs between the two harnesses. `builders` may leak: they hand the
// trees and lists they build to the candidate and never take them back, and a
// leak allocated anywhere else is a helper that failed to release its memory.
// `capacity` lists the fixed-capacity arrays the prelude keeps, by function. A
// new one fails the audit until it is listed with the bound that makes it safe
// or marked KNOWN, and one listed that is no longer there fails it too.
const LANGUAGES = {
  c: {
    compiler: "cc",
    missing: available("cc"),
    unusable: unusable("cc", "c"),
    flags: ["-std=c17"],
    sizes: { flat: 4097, nested: 1025, large: 100000 },
    builders: [
      "main",
      "treeNode",
      "listNode",
      "listNodeCycle",
      "randomList",
      "graphNode",
      "nextTree",
    ],
    capacity: {
      // KNOWN: findTreeNode queues every node of its tree on the stack, so a
      // tree past 4096 nodes writes beyond it. Delete this entry with the fix;
      // the audit fails once the array is gone, so the entry cannot outlive it.
      findTreeNode: ["queue[4096]"],
      // Nodes are added only while count < 1024 and every ID is checked below
      // 1024.
      json_graph: ["nodes[1024]", "seen[1024]"],
    },
  },
  cpp: {
    compiler: "g++",
    missing: available("g++"),
    unusable: unusable("g++", "cpp"),
    flags: ["-std=c++20"],
    sizes: { flat: 129, nested: 129, large: null },
    builders: [
      "main",
      "treeNode",
      "listNode",
      "randomList",
      "graphNode",
      "nextTree",
    ],
    capacity: {},
  },
};

// Every harness carries every helper and every case declares what its call
// does not read, so the unused-name warnings are about the shape of the
// program, not its code.
const WARNINGS = [
  "-Wall",
  "-Wextra",
  "-Werror",
  "-Wno-unused-function",
  "-Wno-unused-variable",
  "-Wno-unused-parameter",
  "-Wno-unused-but-set-variable",
];
const SANITIZE = [
  "-fsanitize=address,undefined",
  "-fno-sanitize-recover=undefined",
];

// An array declared with a literal size: a type, then a name and its
// dimensions, then what ends a declarator. The type is any identifier but a
// keyword that can stand before an index, so a typedef'd element type is seen
// and `return queue[1024];` is not. Capacities below MIN_CAPACITY are scratch
// space, such as a number's digits.
const DECLARED_ARRAY =
  /\b(?!return\b|else\b|case\b|sizeof\b)[A-Za-z_]\w*(?:\s+\**\s*|\s*\*+\s*)(\w+)((?:\[\d+\])+)\s*[;=,]/g;
const MIN_CAPACITY = 64;

// Each case's inputs hold one size, and one program carries all of a judge's
// cases. The sizes are a language's `sizes`, and why they differ is what each
// compiler costs. C reads an array literal of any length quickly, so its flat
// inputs cross a power of two a helper could be sized to (4097), a tree or a
// list also runs at the largest the bank allows (100000), and a nested input,
// which spells out every row, stops at 1025 to cross the 1024 bound a helper
// keeps. g++ takes tens of seconds over a literal that long (a 52 KB nested
// one timed out at 40 s, and so did every 100000-node tree), and its helpers
// grow vectors, so C++ crosses the 64 they start at and the next two doublings
// (129) and goes no higher.
const LARGE_TYPES = new Set(["binaryTree", "linkedList", "nextTree"]);
// Types with nothing to scale: the case's own value stands.
const UNSCALED = new Set(["integer", "double", "boolean", "cyclePos"]);

const isMatrix = (type) => /\[\]\[\]$|^list<list<.+>>$/.test(type);

// A value of `type` that holds `size` items. Contents are fixed by position,
// not by what a judge accepts: the stub reads nothing, and a harness that
// refuses the input must say so in its output rather than crash. A type this
// does not know is an error, so a type the harness gains cannot pass unscaled.
function scaled(type, size, nestedSize, value) {
  if (UNSCALED.has(type)) return value;
  if (type === "treeNodeValue") return size - 1;
  const nested =
    isMatrix(type) ||
    ["randomList", "graphNode", "linkedListArray"].includes(type);
  const n = nested ? Math.min(size, nestedSize) : size;
  if (type === "string") return "a".repeat(n);
  const items = Array.from({ length: n }, (_, i) => i);
  if (type === "randomList") return items.map((i) => [i, 0]);
  if (type === "graphNode") return items.map((i) => [((i + 1) % n) + 1]);
  if (type === "linkedListArray") return items.map((i) => [i]);
  if (isMatrix(type)) {
    // Rows keep the width of the case's own, since a pair is read as a pair.
    const cell = /string|character/.test(type) ? "a" : 0;
    const width = Array.isArray(value?.[0]) ? value[0].length || 1 : 1;
    return items.map(() => Array.from({ length: width }, () => cell));
  }
  if (/^(string|character)\[\]$|^list<string>$/.test(type))
    return items.map(() => "a");
  if (/\[\]$|^list<.+>$|^(linkedList|binaryTree|nextTree)$/.test(type))
    return items;
  throw new Error(`The audit has no way to scale a ${type}`);
}

function sizesFor(spec, sizes) {
  const large = spec.argTypes?.some((type) => LARGE_TYPES.has(type));
  return large && sizes.large ? [sizes.flat, sizes.large] : [sizes.flat];
}

function boundaryCases(spec, sizes) {
  const first = spec.cases[0];
  return sizesFor(spec, sizes).map((size) => ({
    label: `${size} items`,
    input: first.input.map((value, index) =>
      scaled(
        spec.argTypes?.[index] ?? spec.paramTypes[index],
        size,
        sizes.nested,
        value,
      ),
    ),
  }));
}

// The allocator entry points a report lists above the function that called
// them, and the library frames under a C++ container's allocation. GCC's
// runtime names its source path and Clang's names only the library, so the
// name decides, not where the frame is from.
const ALLOCATORS =
  /^(malloc|calloc|realloc|strdup|strndup|posix_memalign|aligned_alloc|operator|__interceptor_\w+|(std|__gnu_cxx)::.*)$/;

/// The function a stack frame names, or undefined for a line that is not one.
/// A frame is `#N 0xADDR in NAME LOCATION`, where NAME is a bare identifier in C
/// and a demangled signature in C++, such as `void std::vector<int>::push_back
/// <int>(int&&) (/usr/lib/libstdc++.so+0x1)`. A signature carries its return
/// type and template arguments, so those are dropped before the name is read.
function frameName(line) {
  const text = line.match(/^\s+#\d+ 0x[0-9a-f]+ in (.+)$/)?.[1];
  if (!text) return undefined;
  let bare = text;
  for (let before = ""; before !== bare;) {
    before = bare;
    bare = bare.replace(/<[^<>]*>/g, "");
  }
  bare = bare.includes("(") ? bare.split("(")[0].trim() : bare.split(" ")[0];
  return /^operator\b/.test(bare) ? "operator" : bare.split(/\s+/).pop();
}

/// The function that allocated each leak in an AddressSanitizer report: the
/// first frame of its stack that is not the sanitizer's own allocator.
function leakOwners(report) {
  const owners = new Set();
  const stacks = report
    .split(/\n(?=\w+ leak of )/)
    .filter((block) => /^\w+ leak of /.test(block));
  for (const stack of stacks) {
    const frame = stack
      .split("\n")
      .filter((line) => !/libsanitizer|\/asan\/|libclang_rt/.test(line))
      .map(frameName)
      .find((name) => name && !ALLOCATORS.test(name));
    // A leak with no frame to name cannot be excused as an input builder.
    owners.add(frame ?? "(unknown)");
  }
  return owners;
}

describe("leak report parsing", () => {
  const leak = (...frames) =>
    [
      "Direct leak of 8 byte(s) in 1 object(s) allocated from:",
      ...frames.map((frame, i) => `    #${i} 0x55d0c0a1b2c3 in ${frame}`),
    ].join("\n");
  const owners = (...frames) => [...leakOwners(leak(...frames))];

  it("names the C function that called the allocator", () => {
    assert.deepEqual(
      owners(
        "malloc ../../../src/asan_malloc_linux.cpp:69",
        "treeNode /w/h.c:19",
      ),
      ["treeNode"],
    );
    assert.deepEqual(
      owners("calloc (/lib/libasan.so.6+0x1)", "listNode (/w/harness+0x2f)"),
      ["listNode"],
    );
  });

  it("looks past a C++ allocator and the library under it", () => {
    assert.deepEqual(
      owners(
        "operator new(unsigned long) ../../../src/asan_new_delete.cpp:99",
        "std::__new_allocator<TreeNode*>::allocate(unsigned long, void const*) /usr/include/c++/13/bits/new_allocator.h:151",
        "void std::vector<int, std::allocator<int> >::emplace_back<int>(int&&) (/w/harness+0x1)",
        "toJson(std::vector<int, std::allocator<int> > const&) (/w/harness+0x2)",
      ),
      ["toJson"],
    );
  });

  it("keeps a function whose arguments are standard types", () => {
    assert.deepEqual(
      owners(
        "operator new(unsigned long) (/lib/libasan.so+0x1)",
        "treeNode(std::vector<std::optional<int>, std::allocator<std::optional<int> > >) (/w/harness+0x3)",
      ),
      ["treeNode"],
    );
  });

  it("does not excuse a leak with no frame to name", () => {
    assert.deepEqual(owners("malloc (/lib/libasan.so+0x1)"), ["(unknown)"]);
  });
});

// What a judge makes the harness generate code for: each parameter's type, the
// return and output types, and the fields that change how a call is made.
function features(spec) {
  const typeOf = (index) => spec.argTypes?.[index] ?? spec.paramTypes[index];
  return [
    // How the answer is read back is a pair, not two facts: a judge that
    // changes an array in place and one that changes a tree in place take
    // different paths, though each type is shown by some other judge.
    `call:${spec.returnType}:${spec.outputType}`,
    ...(spec.outputParam != null
      ? [`in-place:${typeOf(spec.outputParam)}:${spec.outputType}`]
      : []),
    ...(spec.outputPrefixParam != null
      ? [`prefix:${typeOf(spec.outputPrefixParam)}`]
      : []),
    ...spec.paramTypes.map((type, i) => `param:${spec.argTypes?.[i] ?? type}`),
    `return:${spec.returnType}`,
    `output:${spec.outputType}`,
    `checker:${spec.checker}`,
    ...["outputParam", "outputPrefixParam", "cyclePosParam"]
      .filter((field) => spec[field] != null)
      .map((field) => `field:${field}`),
  ];
}

/// The fewest judges, taken greedily, that between them show every feature
/// once; a judge in `keep` is always taken. A feature is what makes main
/// differ from one judge to the next. Building one program per shape took about
/// three times the CPU for the same features, and a C++ program costs several seconds
/// of it.
function coverJudges(entries, keep) {
  const chosen = entries.filter(([id]) => keep(id));
  const covered = new Set(chosen.flatMap(([, spec]) => features(spec)));
  const rest = entries.filter(([id]) => !keep(id));
  while (rest.length) {
    let best = 0;
    let gain = -1;
    rest.forEach(([, spec], index) => {
      const fresh = new Set(features(spec).filter((f) => !covered.has(f)));
      if (fresh.size > gain) [best, gain] = [index, fresh.size];
    });
    if (gain <= 0) break;
    const [picked] = rest.splice(best, 1);
    features(picked[1]).forEach((f) => covered.add(f));
    chosen.push(picked);
  }
  return chosen;
}

// Judges whose harness fails this audit today, by language and id, with the
// problem. Each runs expecting the failure, so fixing one fails here until its
// line is deleted. The pattern is what the failure must look like, so an
// unrelated one (a compile error, a timeout) cannot keep an entry alive after
// its bug is fixed.
const KNOWN_BROKEN = new Map([
  [
    "c:lowest-common-ancestor-of-a-binary-tree",
    {
      reason:
        "findTreeNode overruns its stack queue on a tree of more than 4096 nodes",
      failure:
        /index 4096 out of bounds for type 'TreeNode \*\[4096\]'|AddressSanitizer: stack-buffer-overflow[\s\S]*? in findTreeNode/,
    },
  ],
]);

// A run with no compiler or no sanitizer skips every audit and still exits 0,
// which on a machine that is meant to run it is a green gate that checked
// nothing. CI is such a machine; a developer's is not, and sees the skip.
describe(
  "harness audit prerequisites",
  { skip: process.env.CI === "true" ? false : "enforced where CI is set" },
  () => {
    for (const [language, config] of Object.entries(LANGUAGES))
      it(`${language} has a compiler and a working sanitizer`, () =>
        assert.equal(config.unusable, false));
  },
);

for (const [language, config] of Object.entries(LANGUAGES)) {
  describe(`${language} harness prelude`, { skip: config.missing }, () => {
    it("compiles without warnings", () =>
      withTempDir("codetrial-prelude-", async (dir) => {
        const source = join(dir, `prelude.${language}`);
        writeFileSync(source, prelude(language));
        await run(config.compiler, [
          ...config.flags,
          ...WARNINGS,
          "-c",
          source,
          "-o",
          join(dir, "prelude.o"),
        ]);
      }));

    it("keeps no unlisted fixed-capacity array", () => {
      const found = {};
      let name = "(file scope)";
      for (const line of prelude(language).split("\n")) {
        name =
          line.match(/^[A-Za-z_][^=;]*?\b(\w+)\([^;]*\)\s*\{/)?.[1] ?? name;
        for (const decl of line.matchAll(DECLARED_ARRAY)) {
          const slots = [...decl[2].matchAll(/\d+/g)].reduce(
            (total, dim) => total * Number(dim[0]),
            1,
          );
          if (slots >= MIN_CAPACITY)
            (found[name] ??= []).push(`${decl[1]}${decl[2]}`);
        }
      }
      assert.deepEqual(found, config.capacity);
    });
  });

  describe(
    `${language} harness on every judge`,
    {
      skip: config.unusable,
      concurrency: Math.min(8, availableParallelism()),
    },
    () => {
      const flags = [...config.flags, ...SANITIZE];
      // gcc loads a precompiled header from an include directory in place of
      // the first header a file includes, and the harness opens with the same
      // standard headers every time. Without it each C++ program spends most
      // of its compile time reading them. A directory that fails to build is
      // not an error: the programs compile as they are, only slower.
      let pch = [];
      let pchDir;
      before(async () => {
        if (language !== "cpp") return;
        const headers = prelude(language).match(/^#include <[^>]+>$/gm) ?? [];
        if (!headers.length) return;
        pchDir = mkdtempSync(join(tmpdir(), "codetrial-audit-pch-"));
        try {
          writeFileSync(join(pchDir, "all.h"), `${headers.join("\n")}\n`);
          const first = headers[0].slice("#include <".length, -1);
          await run(config.compiler, [
            ...flags,
            "-x",
            "c++-header",
            join(pchDir, "all.h"),
            "-o",
            join(pchDir, `${first}.gch`),
          ]);
          pch = ["-I", pchDir];
        } catch {
          pch = [];
        }
      });
      after(() => {
        if (pchDir) rmSync(pchDir, { recursive: true, force: true });
      });

      // The prelude is shared; what differs between judges is the code main
      // generates from their types and fields, so the audit takes enough
      // judges to show every feature once, and a judge it found broken always
      // runs. Class judges call a class the candidate defines, which no stub
      // here stands in for.
      const isKnown = (id) => KNOWN_BROKEN.has(`${language}:${id}`);
      const functions = Object.entries(loadJudges()).filter(
        ([, spec]) => spec.kind === "function",
      );
      const judges = coverJudges(functions, isKnown);

      it("audits enough judges to show every feature", () => {
        assert.ok(judges.length >= 15, `only ${judges.length} judges`);
        const shown = new Set(judges.flatMap(([, spec]) => features(spec)));
        for (const [, spec] of functions)
          for (const feature of features(spec))
            assert.ok(shown.has(feature), `${feature} is not audited`);
        for (const key of KNOWN_BROKEN.keys())
          if (key.startsWith(`${language}:`))
            assert.ok(
              judges.some(([id]) => `${language}:${id}` === key),
              `KNOWN_BROKEN names ${key}, which is not audited`,
            );
      });

      const audit = (id, spec) =>
        withTempDir(`codetrial-audit-${id}-`, async (dir) => {
          const source = join(dir, `harness.${language}`);
          const binary = join(dir, "harness");
          writeFileSync(
            source,
            generateHarness(
              language,
              { ...spec, cases: boundaryCases(spec, config.sizes) },
              stubCandidate(spec, language),
            ),
          );
          await run(config.compiler, [
            ...flags,
            ...WARNINGS,
            ...pch,
            source,
            "-o",
            binary,
          ]);
          const { stdout, stderr } = await run(binary, [], {
            timeout: 60000,
            maxBuffer: 1 << 24,
            env: { ...process.env, ...LEAK_ENV },
          });
          // A harness that throws reports the case with an error and exits
          // cleanly, so a run that reached nothing has the right count.
          const { results } = parseCompilerResults(stdout);
          assert.equal(results.length, sizesFor(spec, config.sizes).length);
          assert.deepEqual(
            results.filter((result) => result.error !== undefined),
            [],
            "a case reported an error",
          );
          const builders = new Set(config.builders);
          const strays = [...leakOwners(stderr)].filter(
            (name) => !builders.has(name),
          );
          assert.deepEqual(
            strays,
            [],
            "leaked by a helper that owns its memory",
          );
        });

      for (const [id, spec] of judges) {
        const known = KNOWN_BROKEN.get(`${language}:${id}`);
        it(`${id} survives large inputs`, async () => {
          if (!known) return audit(id, spec);
          await assert.rejects(
            audit(id, spec),
            { stderr: known.failure },
            `${id} now passes; delete its KNOWN_BROKEN entry (${known.reason})`,
          );
        });
      }
    },
  );
}
