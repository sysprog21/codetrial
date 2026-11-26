import assert from "node:assert/strict";
import test, { describe } from "node:test";
import { runInNewContext } from "node:vm";
import { ORIGINAL_TREE_NODE_ERROR } from "../../web/compiler-explorer.js";
import { checkAnswer } from "../../web/lib.js";
import {
  read,
  jsWorkerSource,
  runnerSpec,
  runCompiled,
  compiledSkip,
  runPythonDriver,
  pythonSkip,
} from "./source.js";

const judge = JSON.parse(read("web/judges/shared-approver-lookup.json"));
const worker = jsWorkerSource();
const skip = (language) =>
  language === "javascript"
    ? false
    : language === "python"
      ? pythonSkip()
      : compiledSkip(language);

const helpers = {
  c: `struct TreeNode* find(struct TreeNode* r, struct TreeNode* p, struct TreeNode* q) {
    if (!r || r == p || r == q) return r;
    struct TreeNode* a = find(r->left, p, q); struct TreeNode* b = find(r->right, p, q);
    return a && b ? r : a ? a : b;
  }
  struct TreeNode* copy(struct TreeNode* r) {
    if (!r) return NULL;
    struct TreeNode* n = malloc(sizeof(*n));
    n->val = r->val; n->left = copy(r->left); n->right = copy(r->right); return n;
  }`,
  cpp: `TreeNode* find(TreeNode* r, TreeNode* p, TreeNode* q) {
    if (!r || r == p || r == q) return r;
    auto* a = find(r->left, p, q); auto* b = find(r->right, p, q);
    return a && b ? r : a ? a : b;
  }
  TreeNode* copy(TreeNode* r) { return r ? new TreeNode(r->val, copy(r->left), copy(r->right)) : nullptr; }`,
  java: `TreeNode find(TreeNode r, TreeNode p, TreeNode q) {
    if (r == null || r == p || r == q) return r;
    TreeNode a = find(r.left, p, q), b = find(r.right, p, q);
    return a != null && b != null ? r : a != null ? a : b;
  }
  TreeNode copy(TreeNode r) {
    if (r == null) return null;
    TreeNode n = new TreeNode(r.val); n.left = copy(r.left); n.right = copy(r.right); return n;
  }`,
  python: `def find(r, p, q):
    if r is None or r is p or r is q: return r
    a, b = find(r.left, p, q), find(r.right, p, q)
    return r if a is not None and b is not None else a or b
def copy(r):
    return TreeNode(r.val, copy(r.left), copy(r.right)) if r else None
`,
  javascript: `function find(r, p, q) {
    if (!r || r === p || r === q) return r;
    const a = find(r.left, p, q), b = find(r.right, p, q); return a && b ? r : a || b;
  }
  function copy(r) { return r ? new self.TreeNode(r.val, copy(r.left), copy(r.right)) : null; }`,
};

function candidate(language, mode) {
  const pointer = language === "c" || language === "cpp";
  const left = pointer ? "root->left" : "root.left";
  const value = pointer ? "root->val" : "root.val";
  const nil =
    language === "c" ? "NULL" : language === "cpp" ? "nullptr" : "null";
  let body =
    mode === "copied" || mode === "control"
      ? "return copy(found);"
      : "return found;";
  if (mode === "attached") body = `${left} = copy(found); return ${left};`;
  if (mode === "detached") body = `${left} = ${nil}; return found;`;
  if (mode === "continued")
    body = `if (${value} == 10) return copy(found); return found;`;
  const control = mode === "control";
  if (language === "python") {
    if (mode === "attached")
      body = "root.left = copy(found)\n    return root.left";
    else if (mode === "detached") body = "root.left = None\n    return found";
    else if (mode === "continued")
      body = "return copy(found) if root.val == 10 else found";
    else if (mode === "null") body = "return None";
    return (
      helpers.python +
      `def nearestSharedApprover(${control ? "root" : "root, p, q"}):\n    found = ${control ? "root" : "find(root, p, q)"}\n    ${body}\n`
    );
  }
  const found = control ? "root" : "find(root, p, q)";
  if (mode === "null") body = `return ${nil};`;
  if (language === "javascript")
    return (
      helpers.javascript +
      `self.__codetrialEntry = (root, p, q) => { const found = ${found}; ${body} };`
    );
  const type =
    language === "c"
      ? "struct TreeNode*"
      : language === "cpp"
        ? "TreeNode*"
        : "TreeNode";
  const params = (control ? ["root"] : ["root", "p", "q"])
    .map((name) => `${type} ${name}`)
    .join(", ");
  const method = `${type} nearestSharedApprover(${params}) { ${type} found = ${found}; ${body} }`;
  return language === "c"
    ? helpers.c + method
    : `class Solution { ${language === "cpp" ? "public:" : ""} ${helpers[language]} ${method} };`;
}

async function execute(language, spec, code) {
  if (language === "javascript") {
    let response;
    const self = {
      postMessage: (message) => {
        response = message;
      },
    };
    runInNewContext(code + worker, { self, performance });
    self.onmessage({ data: { spec: runnerSpec(spec), name: spec.entry } });
    return JSON.parse(JSON.stringify(response)).results;
  }
  return language === "python"
    ? runPythonDriver(code, spec).results
    : runCompiled(language, spec, code);
}

describe("ancestor node identity", { concurrency: 3 }, () => {
  test("javascript missing returns are wrong answers without identity errors", async () => {
    for (const body of ["", "return;", "return undefined;"]) {
      const results = await execute(
        "javascript",
        judge,
        `self.__codetrialEntry = (root, p, q) => { ${body} };`,
      );
      assert.equal(results.length, judge.cases.length);
      results.forEach((result, index) => {
        assert.equal(result.error, undefined);
        assert.deepEqual(result.actual, []);
        assert.equal(
          checkAnswer(judge, judge.cases[index], result.actual),
          false,
        );
      });
    }
  });
  for (const language of ["cpp", "java"]) {
    test(
      `${language} identity checks ignore candidate lookup helpers`,
      { skip: skip(language) },
      async () => {
        for (const mode of ["original", "copied"]) {
          let code = candidate(language, mode);
          if (language === "cpp") {
            code =
              `
vector<TreeNode*>::iterator find(vector<TreeNode*>::iterator first,
    vector<TreeNode*>::iterator, TreeNode*) { return first; }
` + code;
          } else {
            code =
              `
class ArrayList<T> extends java.util.ArrayList<T> {
  static TreeNode returned;
  public T get(int index) {
    T value = super.get(index);
    if (value instanceof TreeNode && returned != null && ((TreeNode) value).val == returned.val)
      return (T) returned;
    return value;
  }
  public java.util.stream.Stream<T> stream() {
    return java.util.stream.Stream.of((T) returned);
  }
}
` +
              code.replace(
                mode === "original" ? "return found;" : "return copy(found);",
                `TreeNode result = ${mode === "original" ? "found" : "copy(found)"}; ArrayList.returned = result; return result;`,
              );
          }
          const results = await execute(language, judge, code);
          assert.equal(results.length, judge.cases.length);
          results.forEach((result, index) => {
            if (mode === "copied") {
              assert.ok(
                result.error?.endsWith(ORIGINAL_TREE_NODE_ERROR),
                result.error,
              );
            } else {
              assert.equal(result.error, undefined);
              assert.equal(
                checkAnswer(judge, judge.cases[index], result.actual),
                true,
              );
            }
          });
        }
      },
    );
  }
  for (const language of ["cpp", "java"]) {
    test(
      `${language} candidate timing excludes ancestor validation`,
      { skip: skip(language) },
      async () => {
        // The clock advances one millisecond per read and 100 during validation.
        // This pins the measured interval without depending on host speed.
        const timing =
          language === "cpp"
            ? `
long timerTicks = 0;
namespace std::chrono {
struct AncestorTestClock {
  static steady_clock::time_point now() {
    return steady_clock::time_point(milliseconds(timerTicks++));
  }
};
}
#define steady_clock AncestorTestClock
`
            : `
class System {
  static long ticks;
  static final java.io.PrintStream out = java.lang.System.out;
  static long nanoTime() { return ticks++ * 1000000; }
}
`;
        const results = await runCompiled(
          language,
          judge,
          timing + candidate(language, "original"),
          {
            transformHarness(source) {
              const declaration =
                language === "cpp"
                  ? "bool original = false;"
                  : "boolean original = false;";
              const advance =
                language === "cpp"
                  ? "timerTicks += 100;"
                  : "System.ticks += 100;";
              assert.equal(
                source.split(declaration).length - 1,
                judge.cases.length,
              );
              return source.replaceAll(
                declaration,
                `${advance}\n${declaration}`,
              );
            },
          },
        );
        assert.equal(results.length, judge.cases.length);
        results.forEach((result, index) => {
          assert.equal(result.error, undefined);
          assert.equal(
            checkAnswer(judge, judge.cases[index], result.actual),
            true,
          );
          assert.equal(result.timeMs, 1);
        });
      },
    );
  }
  for (const language of ["javascript", "python"]) {
    for (const helper of ["membership", "boolean"]) {
      test(
        `${language} identity checks ignore overridden ${helper} helpers`,
        { skip: skip(language) },
        async () => {
          for (const mode of ["original", "copied"]) {
            let code = candidate(language, mode);
            if (helper === "boolean") {
              code =
                (language === "javascript"
                  ? "globalThis.Boolean = () => false;\n"
                  : "import builtins\nbuiltins.bool = lambda *args: False\n") +
                code;
            } else if (language === "javascript") {
              code =
                `const savedHas = Set.prototype.has;
function countNodes(root) { return root ? 1 + countNodes(root.left) + countNodes(root.right) : 0; }
` +
                code.replace(
                  "const found =",
                  `const count = countNodes(root);
Set.prototype.has = function(node) { return this.size === count || savedHas.call(this, node); };
const found =`,
                );
            } else {
              code =
                "import builtins\n" +
                code.replace(
                  "    found =",
                  "    builtins.any = lambda *args: True\n    found =",
                );
            }
            const results = await execute(language, judge, code);
            assert.equal(results.length, judge.cases.length);
            results.forEach((result, index) => {
              if (mode === "copied") {
                assert.ok(
                  result.error?.endsWith(ORIGINAL_TREE_NODE_ERROR),
                  result.error,
                );
              } else {
                assert.equal(result.error, undefined);
                assert.equal(
                  checkAnswer(judge, judge.cases[index], result.actual),
                  true,
                );
              }
            });
          }
        },
      );
    }
  }
  for (const language of ["c", "cpp", "java", "python", "javascript"]) {
    test(
      `${language} null ancestors are wrong answers without identity errors`,
      { skip: skip(language) },
      async () => {
        const results = await execute(
          language,
          judge,
          candidate(language, "null"),
        );
        assert.equal(results.length, judge.cases.length);
        results.forEach((result, index) => {
          assert.equal(result.error, undefined);
          assert.deepEqual(result.actual, []);
          assert.equal(
            checkAnswer(judge, judge.cases[index], result.actual),
            false,
          );
        });
      },
    );
    test(
      `${language} checks returned ancestor identity before serialization`,
      { skip: skip(language) },
      async () => {
        for (const mode of ["original", "copied"]) {
          const results = await execute(
            language,
            judge,
            candidate(language, mode),
          );
          assert.equal(results.length, judge.cases.length);
          results.forEach((result, index) => {
            if (mode === "copied")
              assert.ok(
                result.error?.endsWith(ORIGINAL_TREE_NODE_ERROR),
                result.error,
              );
            else {
              assert.equal(result.error, undefined);
              assert.equal(
                checkAnswer(judge, judge.cases[index], result.actual),
                true,
              );
            }
          });
        }
        const cases = [
          { input: [[10, 5, 20, 2, 7], 2, 7] },
          { input: [[11, 5, 20, 2, 7], 2, 7], expected: [5, 2, 7] },
        ];
        for (const mode of ["attached", "detached", "continued"]) {
          const spec = { ...judge, cases };
          const results = await execute(
            language,
            spec,
            candidate(language, mode),
          );
          assert.equal(results.length, 2);
          results.forEach((result, index) => {
            if (mode === "attached" || (mode === "continued" && index === 0))
              assert.ok(
                result.error?.endsWith(ORIGINAL_TREE_NODE_ERROR),
                result.error,
              );
            else {
              assert.equal(result.error, undefined);
              assert.deepEqual(result.actual, [5, 2, 7]);
            }
          });
        }
        const control = {
          ...judge,
          paramNames: ["root"],
          paramTypes: ["TreeNode"],
          argTypes: ["binaryTree"],
          cases: [{ input: [[10, 5, 20]], expected: [10, 5, 20] }],
        };
        const results = await execute(
          language,
          control,
          candidate(language, "control"),
        );
        assert.equal(results.length, 1);
        assert.equal(results[0].error, undefined);
        assert.deepEqual(results[0].actual, control.cases[0].expected);
      },
    );
  }
});
