const ANSI_PATTERN = /\x1b\[[0-?]*[ -/]*[@-~]/g;

const CPP_TYPES = {
  boolean: "bool",
  character: "char",
  double: "double",
  integer: "int",
  string: "string",
  void: "void",
  ListNode: "ListNode*",
  Node: "Node*",
  TreeNode: "TreeNode*",
};

const C_TYPES = {
  boolean: "bool",
  character: "char",
  double: "double",
  integer: "int",
  string: "char*",
  void: "void",
  ListNode: "struct ListNode*",
  Node: "struct Node*",
  TreeNode: "struct TreeNode*",
};

const JAVA_TYPES = {
  boolean: "boolean",
  character: "char",
  double: "double",
  integer: "int",
  string: "String",
  void: "void",
  ListNode: "ListNode",
  Node: "Node",
  TreeNode: "TreeNode",
};

export function stripAnsi(text) {
  return String(text || "").replace(ANSI_PATTERN, "");
}

export function compilerType(type, language) {
  if (type.endsWith("[]")) {
    const item = compilerType(type.slice(0, -2), language);
    if (language === "c") return `${item}*`;
    return language === "cpp" ? `vector<${item}>` : `${item}[]`;
  }
  const list = type.match(/^list<(.+)>$/);
  if (list) {
    const item = compilerType(list[1], language);
    if (language === "c") return `${item}*`;
    return language === "cpp" ? `vector<${item}>` : `List<${boxedJavaType(item)}>`;
  }
  const mapped = ({ c: C_TYPES, cpp: CPP_TYPES, java: JAVA_TYPES }[language] || {})[type];
  if (!mapped) throw new Error(`Unsupported ${language} type: ${type}`);
  return mapped;
}

export function nativeLiteral(value, type, language) {
  if (type.endsWith("[]")) return arrayLiteral(value, type.slice(0, -2), language);
  const list = type.match(/^list<(.+)>$/);
  if (list) return listLiteral(value, list[1], language);
  if (type === "string") return JSON.stringify(value);
  if (type === "character") return charLiteral(value);
  if (type === "boolean") return value ? "true" : "false";
  if (type === "integer" || type === "double") return String(value);
  if (type === "ListNode") return helperCall("listNode", value, "integer[]", language);
  if (type === "Node") throw new Error("Node literals require argTypes preparation");
  if (type === "ListNode[]") return arrayLiteral(value, "ListNode", language);
  if (type === "TreeNode") return treeNodeLiteral(value, language);
  throw new Error(`Unsupported literal type: ${type}`);
}

export function generateHarness(language, spec, candidateCode) {
  if (!["c", "cpp", "java"].includes(language)) throw new Error(`Unsupported harness language: ${language}`);
  if (spec.kind === "class" && language === "c") throw new Error("Only function judges are supported for C");
  if (spec.kind === "class") return language === "cpp" ? cppClassHarness(spec, candidateCode) : javaClassHarness(spec, candidateCode);
  if (spec.kind !== "function") throw new Error("Only function judges are supported");
  if (language === "c") return cHarness(spec, candidateCode);
  return language === "cpp" ? cppHarness(spec, candidateCode) : javaHarness(spec, candidateCode);
}

export function parseCompilerResults(stdout) {
  const text = compilerText(stdout);
  for (let index = text.lastIndexOf("{"); index >= 0; index = text.lastIndexOf("{", index - 1)) {
    try {
      const parsed = JSON.parse(text.slice(index).trim());
      if (Array.isArray(parsed.results)) return parsed;
    } catch {
      // Keep scanning earlier braces; candidate debug output may contain JSON.
    }
    if (index === 0) break;
  }
  return { setupError: "The run produced no JSON results." };
}

export function mapCompilerResponse(response) {
  if (!response) return { setupError: "Compiler Explorer did not return a response." };
  if (response.timedOut) return { setupError: "Compiler Explorer timed out before the run completed." };
  if (response.didExecute === false) {
    const diagnostics = compilerText(response.buildResult?.stderr || response.stderr);
    return { setupError: diagnostics || "Compilation failed before the tests could run." };
  }
  if (response.code && response.code !== 0) {
    const diagnostics = compilerText(response.stderr);
    return { setupError: diagnostics || `Compiler Explorer exited with status ${response.code}.` };
  }
  return parseCompilerResults(response.stdout);
}

function cHarness(spec, candidateCode) {
  return `#include <limits.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
struct ListNode { int val; struct ListNode* next; };
struct TreeNode { int val; struct TreeNode* left; struct TreeNode* right; };
struct Node { int val; struct Node* next; struct Node* random; struct Node** neighbors; int neighborsSize; struct Node* left; struct Node* right; bool isLeaf; struct Node* topLeft; struct Node* topRight; struct Node* bottomLeft; struct Node* bottomRight; };
static struct Node* originalRandomNodes[10000];
static int originalRandomNodesSize = 0;
static struct Node* originalGraphNodes[10000];
static int originalGraphNodesSize = 0;
static void json_string(FILE* out, const char* value) { fputc('"', out); for (const unsigned char* p = (const unsigned char*)(value ? value : ""); *p; ++p) { if (*p == '\\\\' || *p == '"') { fputc('\\\\', out); fputc(*p, out); } else if (*p < 0x20) fprintf(out, "\\\\u%04x", *p); else fputc(*p, out); } fputc('"', out); }
static struct ListNode* listNode(int* values, int size) { struct ListNode dummy = {0, NULL}; struct ListNode* tail = &dummy; for (int i = 0; i < size; i++) { tail->next = malloc(sizeof(*tail->next)); tail = tail->next; tail->val = values[i]; tail->next = NULL; } return dummy.next; }
static struct ListNode* listNodeCycle(int* values, int size, int pos) { struct ListNode* head = listNode(values, size); struct ListNode* tail = NULL; struct ListNode* cycle = NULL; int index = 0; for (struct ListNode* node = head; node; node = node->next, index++) { tail = node; if (index == pos) cycle = node; } if (tail && cycle) tail->next = cycle; return head; }
static struct TreeNode* treeNode(int* values, bool* nulls, int size) { if (!size || nulls[0]) return NULL; struct TreeNode** nodes = calloc(size, sizeof(*nodes)); for (int i = 0; i < size; i++) if (!nulls[i]) { nodes[i] = calloc(1, sizeof(*nodes[i])); nodes[i]->val = values[i]; } for (int i = 0, child = 1; i < size && child < size; i++) { if (!nodes[i]) continue; if (child < size) nodes[i]->left = nodes[child++]; if (child < size) nodes[i]->right = nodes[child++]; } struct TreeNode* root = nodes[0]; free(nodes); return root; }
static struct TreeNode* findTreeNode(struct TreeNode* root, int value) { struct TreeNode* queue[4096]; int head = 0, tail = 0; if (root) queue[tail++] = root; while (head < tail) { struct TreeNode* node = queue[head++]; if (node->val == value) return node; if (node->left) queue[tail++] = node->left; if (node->right) queue[tail++] = node->right; } return NULL; }
static bool isOriginalRandomNode(struct Node* node) { for (int i = 0; i < originalRandomNodesSize; i++) if (originalRandomNodes[i] == node) return true; return false; }
static bool isOriginalGraphNode(struct Node* node) { for (int i = 0; i < originalGraphNodesSize; i++) if (originalGraphNodes[i] == node) return true; return false; }
static struct Node* randomList(int values[][2], int size) { originalRandomNodesSize = size; if (!size) return NULL; struct Node** nodes = calloc(size, sizeof(*nodes)); for (int i = 0; i < size; i++) { nodes[i] = calloc(1, sizeof(*nodes[i])); nodes[i]->val = values[i][0]; originalRandomNodes[i] = nodes[i]; } for (int i = 0; i < size; i++) { nodes[i]->next = i + 1 < size ? nodes[i + 1] : NULL; if (values[i][1] >= 0 && values[i][1] < size) nodes[i]->random = nodes[values[i][1]]; } struct Node* head = nodes[0]; free(nodes); return head; }
static struct Node* graphNode(int** adjacency, int size, int* colSize) { originalGraphNodesSize = size; if (!size) return NULL; struct Node** nodes = calloc(size, sizeof(*nodes)); for (int i = 0; i < size; i++) { nodes[i] = calloc(1, sizeof(*nodes[i])); nodes[i]->val = i + 1; nodes[i]->neighborsSize = colSize[i]; nodes[i]->neighbors = calloc(colSize[i], sizeof(*nodes[i]->neighbors)); originalGraphNodes[i] = nodes[i]; } for (int i = 0; i < size; i++) for (int j = 0; j < colSize[i]; j++) nodes[i]->neighbors[j] = nodes[adjacency[i][j] - 1]; struct Node* first = nodes[0]; free(nodes); return first; }
static struct Node* nextTree(int* values, bool* nulls, int size) { if (!size || nulls[0]) return NULL; struct Node** nodes = calloc(size, sizeof(*nodes)); for (int i = 0; i < size; i++) if (!nulls[i]) { nodes[i] = calloc(1, sizeof(*nodes[i])); nodes[i]->val = values[i]; } for (int i = 0, child = 1; i < size && child < size; i++) { if (!nodes[i]) continue; if (child < size) nodes[i]->left = nodes[child++]; if (child < size) nodes[i]->right = nodes[child++]; } struct Node* root = nodes[0]; free(nodes); return root; }
static void json_int_array(FILE* out, const int* values, int size) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); fprintf(out, "%d", values[i]); } fputc(']', out); }
static void json_double_array(FILE* out, const double* values, int size) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); fprintf(out, "%.17g", values[i]); } fputc(']', out); }
static void json_string_array(FILE* out, char** values, int size) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); json_string(out, values[i]); } fputc(']', out); }
static void json_int_matrix(FILE* out, int** values, int size, int* colSize) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); json_int_array(out, values[i], colSize[i]); } fputc(']', out); }
static void json_char_matrix(FILE* out, char** values, int size, int* colSize) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); fputc('[', out); for (int j = 0; j < colSize[i]; j++) { if (j) fputc(',', out); char item[2] = { values[i][j], 0 }; json_string(out, item); } fputc(']', out); } fputc(']', out); }
static void json_string_matrix(FILE* out, char*** values, int size, int* colSize) { fputc('[', out); for (int i = 0; i < size; i++) { if (i) fputc(',', out); json_string_array(out, values[i], colSize[i]); } fputc(']', out); }
static void json_list(FILE* out, struct ListNode* node) { fputc('[', out); for (int i = 0; node && i < 10000; node = node->next, i++) { if (i) fputc(',', out); fprintf(out, "%d", node->val); } fputc(']', out); }
static void json_tree(FILE* out, struct TreeNode* root) { if (!root) { fputs("[]", out); return; } struct TreeNode* queue[4096]; const char* values[4096]; char numbers[4096][32]; int head = 0, tail = 0, count = 0; queue[tail++] = root; while (head < tail && tail < 4096) { struct TreeNode* node = queue[head++]; if (!node) { values[count++] = "null"; continue; } snprintf(numbers[count], sizeof(numbers[count]), "%d", node->val); values[count] = numbers[count]; count++; queue[tail++] = node->left; queue[tail++] = node->right; } while (count > 0 && strcmp(values[count - 1], "null") == 0) count--; fputc('[', out); for (int i = 0; i < count; i++) { if (i) fputc(',', out); fputs(values[i], out); } fputc(']', out); }
static void json_random_list(FILE* out, struct Node* node) { struct Node* nodes[10000]; int count = 0; for (struct Node* cur = node; cur && count < 10000; cur = cur->next) { if (isOriginalRandomNode(cur)) { json_string(out, "Returned list must be a deep copy."); return; } nodes[count++] = cur; } fputc('[', out); for (int i = 0; i < count; i++) { if (i) fputc(',', out); int random = -1; for (int j = 0; j < count; j++) if (nodes[i]->random == nodes[j]) random = j; fprintf(out, "[%d,", nodes[i]->val); if (nodes[i]->random && random < 0) { json_string(out, "Returned random pointer must stay inside the copied list."); fputc(']', out); continue; } if (random < 0) fputs("null", out); else fprintf(out, "%d", random); fputc(']', out); } fputc(']', out); }
static void json_graph(FILE* out, struct Node* node) { if (!node) { fputs("[]", out); return; } struct Node* nodes[1024]; bool seen[1024] = {0}; int count = 0; nodes[count++] = node; for (int i = 0; i < count && count < 1024; i++) { struct Node* cur = nodes[i]; if (isOriginalGraphNode(cur)) { json_string(out, "Returned graph must be a deep copy."); return; } if (cur->val > 0 && cur->val < 1024) seen[cur->val] = true; for (int j = 0; j < cur->neighborsSize; j++) { struct Node* next = cur->neighbors[j]; if (next && next->val > 0 && next->val < 1024 && !seen[next->val]) { seen[next->val] = true; nodes[count++] = next; } } } fputc('[', out); for (int value = 1, printed = 0; value < 1024; value++) { struct Node* cur = NULL; for (int i = 0; i < count; i++) if (nodes[i]->val == value) cur = nodes[i]; if (!cur) continue; if (printed++) fputc(',', out); fputc('[', out); for (int j = 0; j < cur->neighborsSize; j++) { if (j) fputc(',', out); fprintf(out, "%d", cur->neighbors[j]->val); } fputc(']', out); } fputc(']', out); }
static void json_next_tree(FILE* out, struct Node* root) { if (!root) { fputs("[]", out); return; } struct Node* queue[4096]; int head = 0, tail = 0, firstLevel = 1; queue[tail++] = root; fputc('[', out); while (head < tail) { int end = tail; if (!firstLevel) fputc(',', out); firstLevel = 0; fputc('[', out); for (int i = head; i < end; i++) { if (i > head) fputc(',', out); fprintf(out, "%d", queue[i]->val); if (queue[i]->next != (i + 1 < end ? queue[i + 1] : NULL)) { fputs("]", out); fputs("]", out); return; } if (queue[i]->left) queue[tail++] = queue[i]->left; if (queue[i]->right) queue[tail++] = queue[i]->right; } fputc(']', out); head = end; } fputc(']', out); }
static void json_quad_tree_visit(FILE* out, struct Node* node, int* printed) { if (!node) return; if ((*printed)++) fputc(',', out); fprintf(out, "[%d,%d]", node->isLeaf ? 1 : 0, node->isLeaf ? (node->val ? 1 : 0) : 1); if (!node->isLeaf) { json_quad_tree_visit(out, node->topLeft, printed); json_quad_tree_visit(out, node->topRight, printed); json_quad_tree_visit(out, node->bottomLeft, printed); json_quad_tree_visit(out, node->bottomRight, printed); } }
static void json_quad_tree(FILE* out, struct Node* root) { int printed = 0; fputc('[', out); json_quad_tree_visit(out, root, &printed); fputc(']', out); }
${candidateCode}
int main(void) {
  printf("{\\"results\\":[");
${spec.cases.map((testCase, index) => cCase(spec, testCase, index)).join("\n")}
  printf("]}");
  return 0;
}`;
}

function cCase(spec, testCase, index) {
  const declarations = spec.paramTypes.map((type, paramIndex) => cDeclaration(spec, testCase, type, paramIndex)).join("\n");
  return `  ${index ? "printf(\",\");" : ""}
  {
${declarations}
    int returnSize = 0;
    int* returnColumnSizes = NULL;
    clock_t start = clock();
    ${cCall(spec)}
    double elapsed = (double)(clock() - start) * 1000.0 / CLOCKS_PER_SEC;
    printf("{\\"actual\\":");
    ${cPrintActual(spec)}
    printf(",\\"timeMs\\":%.3f}", elapsed);
  }`;
}

function cCall(spec) {
  const callArgs = cCallArgs(spec).join(", ");
  if (Number.isInteger(spec.outputPrefixParam)) {
    return `int actualSize = ${spec.entry}(${callArgs}); if (actualSize < 0 || actualSize > ${spec.paramNames[spec.outputPrefixParam]}Size) actualSize = 0;`;
  }
  if (spec.returnType === "void") return `${spec.entry}(${callArgs});`;
  return `${cReturnDeclaration(spec.returnType)} actual = ${spec.entry}(${callArgs});`;
}

function cCallArgs(spec) {
  const args = [];
  spec.paramNames.forEach((name, index) => {
    if (spec.argTypes?.[index] === "cyclePos") return;
    const type = spec.paramTypes[index];
    args.push(name);
    if (cNeedsSize(type)) args.push(`${name}Size`);
    if (type.endsWith("[][]") || /^list<list<.+>>$/.test(type)) args.push(`${name}ColSize`);
  });
  if (spec.returnType.endsWith("[]") || /^list<.+>$/.test(spec.returnType)) args.push("&returnSize");
  if (spec.returnType.endsWith("[][]") || /^list<list<.+>>$/.test(spec.returnType)) args.push("&returnColumnSizes");
  return args;
}

function cReturnDeclaration(type) {
  return compilerType(type, "c");
}

function cPrintActual(spec) {
  const name = Number.isInteger(spec.outputParam) ? spec.paramNames[spec.outputParam] : "actual";
  const type = Number.isInteger(spec.outputParam) ? spec.paramTypes[spec.outputParam] : spec.returnType;
  if (Number.isInteger(spec.outputPrefixParam)) return `json_int_array(stdout, ${spec.paramNames[spec.outputPrefixParam]}, actualSize);`;
  if (spec.outputType === "linkedList") return `json_list(stdout, ${name});`;
  if (spec.outputType === "binaryTree") return `json_tree(stdout, ${name});`;
  if (spec.outputType === "randomList") return `json_random_list(stdout, ${name});`;
  if (spec.outputType === "graphNode") return `json_graph(stdout, ${name});`;
  if (spec.outputType === "nextTree") return `json_next_tree(stdout, ${name});`;
  if (spec.outputType === "quadTree") return `json_quad_tree(stdout, ${name});`;
  if (type === "boolean") return `printf(${name} ? "true" : "false");`;
  if (type === "integer") return `printf("%d", ${name});`;
  if (type === "double") return `printf("%.17g", ${name});`;
  if (type === "string") return `json_string(stdout, ${name});`;
  if (type === "integer[]") return `json_int_array(stdout, ${name}, ${Number.isInteger(spec.outputParam) ? `${name}Size` : "returnSize"});`;
  if (type === "double[]") return `json_double_array(stdout, ${name}, ${Number.isInteger(spec.outputParam) ? `${name}Size` : "returnSize"});`;
  if (type === "string[]" || type === "list<string>") return `json_string_array(stdout, ${name}, ${Number.isInteger(spec.outputParam) ? `${name}Size` : "returnSize"});`;
  if (type === "integer[][]" || type === "list<list<integer>>") return `json_int_matrix(stdout, ${name}, ${Number.isInteger(spec.outputParam) ? `${name}Size` : "returnSize"}, ${Number.isInteger(spec.outputParam) ? `${name}ColSize` : "returnColumnSizes"});`;
  if (type === "character[][]") return `json_char_matrix(stdout, ${name}, ${name}Size, ${name}ColSize);`;
  if (type === "list<list<string>>") return `json_string_matrix(stdout, ${name}, returnSize, returnColumnSizes);`;
  if (type === "ListNode") return `json_list(stdout, ${name});`;
  if (type === "TreeNode") return `json_tree(stdout, ${name});`;
  return `printf("null");`;
}

function cppHarness(spec, candidateCode) {
  return `${cppPrelude()}
${candidateCode}
int main() {
  vector<string> results;
${spec.cases.map((testCase, index) => cppCase(spec, testCase, index)).join("\n")}
  cout << "{\\"results\\":["; for (size_t i = 0; i < results.size(); ++i) { if (i) cout << ","; cout << results[i]; } cout << "]}";
}`;
}

function cppPrelude() {
  return `#include <algorithm>
#include <array>
#include <chrono>
#include <climits>
#include <cmath>
#include <deque>
#include <functional>
#include <iomanip>
#include <iostream>
#include <limits>
#include <list>
#include <map>
#include <numeric>
#include <optional>
#include <queue>
#include <set>
#include <sstream>
#include <stack>
#include <stdexcept>
#include <string>
#include <tuple>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>
using namespace std;
struct ListNode { int val; ListNode* next; ListNode(int x=0, ListNode* n=nullptr): val(x), next(n) {} };
struct TreeNode { int val; TreeNode* left; TreeNode* right; TreeNode(int x=0): val(x), left(nullptr), right(nullptr) {} TreeNode(int x, TreeNode* l, TreeNode* r): val(x), left(l), right(r) {} };
class Node { public: int val; bool isLeaf; Node* next; Node* random; vector<Node*> neighbors; Node* left; Node* right; Node* topLeft; Node* topRight; Node* bottomLeft; Node* bottomRight; Node(int x=0): val(x), isLeaf(false), next(nullptr), random(nullptr), left(nullptr), right(nullptr), topLeft(nullptr), topRight(nullptr), bottomLeft(nullptr), bottomRight(nullptr) {} Node(int x, bool leaf): Node(x) { isLeaf = leaf; } Node(bool x, bool leaf): Node(x ? 1 : 0, leaf) {} Node(int x, vector<Node*> n): Node(x) { neighbors = n; } Node(int x, Node* l, Node* r, Node* n): Node(x) { next = n; left = l; right = r; } Node(int x, bool leaf, Node* tl, Node* tr, Node* bl, Node* br): Node(x, leaf) { topLeft = tl; topRight = tr; bottomLeft = bl; bottomRight = br; } Node(bool x, bool leaf, Node* tl, Node* tr, Node* bl, Node* br): Node(x ? 1 : 0, leaf, tl, tr, bl, br) {} };
set<Node*> originalRandomNodes;
set<Node*> originalGraphNodes;
string jsonEscape(const string& value) { ostringstream out; out << '"'; for (unsigned char c : value) { if (c == '\\\\' || c == '"') { out << '\\\\' << c; } else if (c < 0x20) { out << "\\\\u" << hex << setw(4) << setfill('0') << (int)c << dec; } else { out << c; } } out << '"'; return out.str(); }
string toJson(bool value) { return value ? "true" : "false"; }
string toJson(char value) { return jsonEscape(string(1, value)); }
string toJson(int value) { return to_string(value); }
string toJson(double value) { if (!isfinite(value)) return "null"; ostringstream out; out << setprecision(17) << value; return out.str(); }
string toJson(const string& value) { return jsonEscape(value); }
template<class T> string toJson(const vector<T>& values) { string out = "["; for (size_t i = 0; i < values.size(); ++i) { if (i) out += ","; out += toJson(values[i]); } return out + "]"; }
string toJson(ListNode* node) { vector<int> values; set<ListNode*> seen; while (node && !seen.count(node)) { seen.insert(node); values.push_back(node->val); node = node->next; } return toJson(values); }
string toJson(TreeNode* root) { if (!root) return "[]"; vector<string> values; queue<TreeNode*> q; q.push(root); while (!q.empty()) { auto* node = q.front(); q.pop(); if (!node) { values.push_back("null"); continue; } values.push_back(to_string(node->val)); q.push(node->left); q.push(node->right); } while (!values.empty() && values.back() == "null") values.pop_back(); string out = "["; for (size_t i = 0; i < values.size(); ++i) { if (i) out += ","; out += values[i]; } return out + "]"; }
ListNode* listNode(vector<int> values) { ListNode dummy; auto* tail = &dummy; for (int value : values) { tail->next = new ListNode(value); tail = tail->next; } return dummy.next; }
ListNode* listNode(vector<int> values, int pos) { ListNode* head = listNode(values); vector<ListNode*> nodes; for (auto* node = head; node; node = node->next) nodes.push_back(node); if (pos >= 0 && pos < (int)nodes.size() && !nodes.empty()) nodes.back()->next = nodes[pos]; return head; }
TreeNode* treeNode(vector<optional<int>> values) { if (values.empty() || !values[0]) return nullptr; auto* root = new TreeNode(*values[0]); queue<TreeNode*> q; q.push(root); size_t i = 1; while (!q.empty() && i < values.size()) { auto* node = q.front(); q.pop(); if (i < values.size() && values[i]) { node->left = new TreeNode(*values[i]); q.push(node->left); } ++i; if (i < values.size() && values[i]) { node->right = new TreeNode(*values[i]); q.push(node->right); } ++i; } return root; }
TreeNode* findTreeNode(TreeNode* root, int value) { queue<TreeNode*> q; if (root) q.push(root); while (!q.empty()) { auto* node = q.front(); q.pop(); if (node->val == value) return node; if (node->left) q.push(node->left); if (node->right) q.push(node->right); } throw runtime_error("Could not find tree node value."); }
Node* randomList(vector<pair<int, optional<int>>> items) { vector<Node*> nodes; for (auto item : items) nodes.push_back(new Node(item.first)); originalRandomNodes = set<Node*>(nodes.begin(), nodes.end()); for (size_t i = 0; i < nodes.size(); ++i) { nodes[i]->next = i + 1 < nodes.size() ? nodes[i + 1] : nullptr; if (items[i].second) nodes[i]->random = nodes[*items[i].second]; } return nodes.empty() ? nullptr : nodes[0]; }
string randomListToJson(Node* node) { vector<Node*> nodes; map<Node*, int> index; set<Node*> seen; for (auto* cur = node; cur && !seen.count(cur); cur = cur->next) { if (originalRandomNodes.count(cur)) throw runtime_error("Returned list must be a deep copy."); seen.insert(cur); index[cur] = nodes.size(); nodes.push_back(cur); } string out = "["; for (size_t i = 0; i < nodes.size(); ++i) { if (i) out += ","; if (nodes[i]->random && !index.count(nodes[i]->random)) throw runtime_error("Returned random pointer must stay inside the copied list."); out += "[" + to_string(nodes[i]->val) + "," + (nodes[i]->random ? to_string(index[nodes[i]->random]) : string("null")) + "]"; } return out + "]"; }
Node* graphNode(vector<vector<int>> adjacency) { if (adjacency.empty()) { originalGraphNodes.clear(); return nullptr; } vector<Node*> nodes; for (size_t i = 0; i < adjacency.size(); ++i) nodes.push_back(new Node(i + 1)); originalGraphNodes = set<Node*>(nodes.begin(), nodes.end()); for (size_t i = 0; i < adjacency.size(); ++i) for (int value : adjacency[i]) nodes[i]->neighbors.push_back(nodes[value - 1]); return nodes[0]; }
string graphToJson(Node* node) { if (!node) return "[]"; map<int, Node*> byValue; queue<Node*> q; q.push(node); while (!q.empty()) { auto* cur = q.front(); q.pop(); if (!cur) continue; if (originalGraphNodes.count(cur)) throw runtime_error("Returned graph must be a deep copy."); if (byValue.count(cur->val)) continue; byValue[cur->val] = cur; for (auto* next : cur->neighbors) q.push(next); } string out = "["; bool first = true; for (auto& [value, cur] : byValue) { if (!first) out += ","; first = false; vector<int> neighbors; for (auto* next : cur->neighbors) neighbors.push_back(next->val); sort(neighbors.begin(), neighbors.end()); out += toJson(neighbors); } return out + "]"; }
Node* nextTree(vector<optional<int>> values) { if (values.empty() || !values[0]) return nullptr; auto* root = new Node(*values[0]); queue<Node*> q; q.push(root); size_t i = 1; while (!q.empty() && i < values.size()) { auto* node = q.front(); q.pop(); if (i < values.size() && values[i]) { node->left = new Node(*values[i]); q.push(node->left); } ++i; if (i < values.size() && values[i]) { node->right = new Node(*values[i]); q.push(node->right); } ++i; } return root; }
string nextTreeToJson(Node* root) { vector<string> levels; queue<Node*> q; if (root) q.push(root); while (!q.empty()) { size_t count = q.size(); vector<Node*> level; for (size_t i = 0; i < count; ++i) { auto* node = q.front(); q.pop(); level.push_back(node); if (node->left) q.push(node->left); if (node->right) q.push(node->right); } vector<int> row; for (size_t i = 0; i < level.size(); ++i) { if (level[i]->next != (i + 1 < level.size() ? level[i + 1] : nullptr)) throw runtime_error("next pointers do not match the tree level order."); row.push_back(level[i]->val); } levels.push_back(toJson(row)); } string out = "["; for (size_t i = 0; i < levels.size(); ++i) { if (i) out += ","; out += levels[i]; } return out + "]"; }
string quadTreeToJson(Node* root) { vector<string> rows; function<void(Node*)> visit = [&](Node* node) { if (!node) return; rows.push_back("[" + to_string(node->isLeaf ? 1 : 0) + "," + to_string(node->isLeaf ? (node->val ? 1 : 0) : 1) + "]"); if (!node->isLeaf) { visit(node->topLeft); visit(node->topRight); visit(node->bottomLeft); visit(node->bottomRight); } }; visit(root); string out = "["; for (size_t i = 0; i < rows.size(); ++i) { if (i) out += ","; out += rows[i]; } return out + "]"; }`;
}

function cppClassHarness(spec, candidateCode) {
  return `${cppPrelude()}
string jsonFragments(const vector<string>& values) { string out = "["; for (size_t i = 0; i < values.size(); ++i) { if (i) out += ","; out += values[i]; } return out + "]"; }
${candidateCode}
int main() {
  vector<string> results;
${spec.cases.map((testCase) => cppClassCase(spec, testCase)).join("\n")}
  cout << "{\\"results\\":["; for (size_t i = 0; i < results.size(); ++i) { if (i) cout << ","; cout << results[i]; } cout << "]}";
}`;
}

function cppCase(spec, testCase) {
  const declarations = spec.paramTypes.map((type, paramIndex) => cppDeclaration(spec, testCase, type, paramIndex)).join("\n");
  const callArgs = callParamNames(spec).join(", ");
  const output = cppActualExpression(spec);
  const call = Number.isInteger(spec.outputPrefixParam)
    ? `auto outputSize = solution.${spec.entry}(${callArgs});\n    ${cppPrefixGuard(spec, "outputSize")}\n    auto actual = ${cppPrefixExpression(spec, "outputSize")};`
    : spec.returnType === "void"
      ? `solution.${spec.entry}(${callArgs});\n    auto actual = ${output};`
      : `auto actual = solution.${spec.entry}(${callArgs});`;
  return `  try {
${declarations}
    Solution solution;
    auto start = chrono::steady_clock::now();
    ${call}
    auto elapsed = chrono::duration<double, milli>(chrono::steady_clock::now() - start).count();
    results.push_back("{\\"actual\\":" + ${cppJsonExpression(spec, "actual")} + ",\\"timeMs\\":" + toJson(elapsed) + "}");
  } catch (const exception& error) {
    results.push_back("{\\"error\\":" + jsonEscape(error.what()) + ",\\"timeMs\\":0}");
  }`;
}

function cppClassCase(spec, testCase) {
  const [methods, argsList] = testCase.input;
  return `  try {
    auto start = chrono::steady_clock::now();
    ${cppClassConstructorDeclarations(spec, argsList[0]).join("\n    ")}
    ${spec.className} instance{${cppClassArgs(argsList[0], spec.constructorArgTypes).join(", ")}};
    vector<string> actual;
    actual.push_back("null");
${methods.slice(1).map((method, index) => cppClassMethodCall(method, argsList[index + 1], testCase.expected[index + 1])).join("\n")}
    auto elapsed = chrono::duration<double, milli>(chrono::steady_clock::now() - start).count();
    results.push_back("{\\"actual\\":" + jsonFragments(actual) + ",\\"timeMs\\":" + toJson(elapsed) + "}");
  } catch (const exception& error) {
    results.push_back("{\\"error\\":" + jsonEscape(error.what()) + ",\\"timeMs\\":0}");
  }`;
}

function cppClassMethodCall(method, args, expected) {
  const call = `instance.${method}(${(args || []).map((value) => cppClassLiteral(value)).join(", ")})`;
  return expected === null
    ? `    ${call};\n    actual.push_back("null");`
    : `    actual.push_back(toJson(${call}));`;
}

function cppClassConstructorDeclarations(spec, args) {
  if (!spec.constructorArgTypes) return [];
  return spec.constructorArgTypes.map((type, index) => {
    if (type === "binaryTree") return `TreeNode* constructorArg${index} = ${treeNodeLiteral(args[index], "cpp")};`;
    return "";
  }).filter(Boolean);
}

function cppClassArgs(args, argTypes) {
  return (args || []).map((value, index) => {
    if (argTypes?.[index] === "binaryTree") return `constructorArg${index}`;
    return cppClassLiteral(value);
  });
}

function cppClassLiteral(value) {
  if (typeof value === "string") return nativeLiteral(value, "string", "cpp");
  if (typeof value === "boolean") return nativeLiteral(value, "boolean", "cpp");
  if (Number.isInteger(value)) return nativeLiteral(value, "integer", "cpp");
  if (typeof value === "number") return nativeLiteral(value, "double", "cpp");
  if (Array.isArray(value)) return nativeLiteral(value, "integer[]", "cpp");
  if (value === null) return "nullptr";
  throw new Error(`Unsupported C++ class literal: ${JSON.stringify(value)}`);
}

function cppActualExpression(spec) {
  if (Number.isInteger(spec.outputParam)) return spec.paramNames[spec.outputParam];
  return "actual";
}

function cppPrefixExpression(spec, lengthName) {
  if (Number.isInteger(spec.outputPrefixParam)) {
    const name = spec.paramNames[spec.outputPrefixParam];
    return `vector(${name}.begin(), ${name}.begin() + ${lengthName})`;
  }
  return lengthName;
}

function cppPrefixGuard(spec, lengthName) {
  const name = spec.paramNames[spec.outputPrefixParam];
  return `if (${lengthName} < 0 || ${lengthName} > (int)${name}.size()) throw runtime_error("Return value must be an integer length for the kept prefix.");`;
}

function javaHarness(spec, candidateCode) {
  candidateCode = candidateCode.replace(/\bpublic\s+class\s+Solution\b/g, "class Solution");
  return `import java.util.*;
class ListNode { int val; ListNode next; ListNode() { this(0, null); } ListNode(int val) { this(val, null); } ListNode(int val, ListNode next) { this.val = val; this.next = next; } }
class TreeNode { int val; TreeNode left; TreeNode right; TreeNode() { this(0, null, null); } TreeNode(int val) { this(val, null, null); } TreeNode(int val, TreeNode left, TreeNode right) { this.val = val; this.left = left; this.right = right; } }
class Node { public int val; public boolean isLeaf; public Node next; public Node random; public List<Node> neighbors; public Node left; public Node right; public Node topLeft; public Node topRight; public Node bottomLeft; public Node bottomRight; public Node() { this(0); } public Node(int val) { this.val = val; this.neighbors = new ArrayList<>(); } public Node(int val, boolean isLeaf) { this(val); this.isLeaf = isLeaf; } public Node(boolean val, boolean isLeaf) { this(val ? 1 : 0, isLeaf); } public Node(int val, ArrayList<Node> neighbors) { this.val = val; this.neighbors = neighbors; } public Node(int val, Node left, Node right, Node next) { this(val); this.left = left; this.right = right; this.next = next; } public Node(int val, boolean isLeaf, Node topLeft, Node topRight, Node bottomLeft, Node bottomRight) { this(val, isLeaf); this.topLeft = topLeft; this.topRight = topRight; this.bottomLeft = bottomLeft; this.bottomRight = bottomRight; } public Node(boolean val, boolean isLeaf, Node topLeft, Node topRight, Node bottomLeft, Node bottomRight) { this(val ? 1 : 0, isLeaf, topLeft, topRight, bottomLeft, bottomRight); } }
${candidateCode}
class Main {
  static Set<Node> originalRandomNodes = new HashSet<>();
  static Set<Node> originalGraphNodes = new HashSet<>();
  static String json(String value) { StringBuilder out = new StringBuilder("\\""); for (char c : value.toCharArray()) { if (c == '\\\\' || c == '"') out.append('\\\\').append(c); else if (c < 0x20) out.append(String.format("\\\\u%04x", (int) c)); else out.append(c); } return out.append('"').toString(); }
  static String json(boolean value) { return value ? "true" : "false"; }
  static String json(int value) { return String.valueOf(value); }
  static String json(double value) { return Double.isFinite(value) ? String.valueOf(value) : "null"; }
  static String json(int[] values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (int value : values) out.add(json(value)); return out.toString(); }
  static String json(double[] values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (double value : values) out.add(json(value)); return out.toString(); }
  static String json(String[] values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (String value : values) out.add(json(value)); return out.toString(); }
  static String json(char[][] values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (char[] row : values) { StringJoiner inner = new StringJoiner(",", "[", "]"); for (char value : row) inner.add(json(String.valueOf(value))); out.add(inner.toString()); } return out.toString(); }
  static String json(int[][] values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (int[] row : values) out.add(json(row)); return out.toString(); }
  static String json(List<?> values) { StringJoiner out = new StringJoiner(",", "[", "]"); for (Object value : values) out.add(jsonAny(value)); return out.toString(); }
  static String jsonAny(Object value) { if (value == null) return "null"; if (value instanceof String s) return json(s); if (value instanceof Integer i) return json(i); if (value instanceof Double d) return json(d); if (value instanceof Boolean b) return json(b); if (value instanceof int[] a) return json(a); if (value instanceof double[] a) return json(a); if (value instanceof String[] a) return json(a); if (value instanceof char[][] a) return json(a); if (value instanceof int[][] a) return json(a); if (value instanceof List<?> l) return json(l); if (value instanceof ListNode n) return json(n); if (value instanceof TreeNode n) return json(n); return json(String.valueOf(value)); }
  static String json(ListNode node) { ArrayList<Integer> values = new ArrayList<>(); HashSet<ListNode> seen = new HashSet<>(); while (node != null && !seen.contains(node)) { seen.add(node); values.add(node.val); node = node.next; } return json(values); }
  static String json(TreeNode root) { if (root == null) return "[]"; ArrayList<String> values = new ArrayList<>(); LinkedList<TreeNode> q = new LinkedList<>(); q.add(root); while (!q.isEmpty()) { TreeNode node = q.remove(); if (node == null) { values.add("null"); continue; } values.add(String.valueOf(node.val)); q.add(node.left); q.add(node.right); } while (!values.isEmpty() && values.get(values.size() - 1).equals("null")) values.remove(values.size() - 1); return "[" + String.join(",", values) + "]"; }
  static ListNode listNode(int[] values) { ListNode dummy = new ListNode(0), tail = dummy; for (int value : values) { tail.next = new ListNode(value); tail = tail.next; } return dummy.next; }
  static ListNode listNode(int[] values, int pos) { ListNode head = listNode(values); ArrayList<ListNode> nodes = new ArrayList<>(); for (ListNode node = head; node != null; node = node.next) nodes.add(node); if (pos >= 0 && pos < nodes.size() && !nodes.isEmpty()) nodes.get(nodes.size() - 1).next = nodes.get(pos); return head; }
  static TreeNode treeNode(Integer[] values) { if (values.length == 0 || values[0] == null) return null; TreeNode root = new TreeNode(values[0]); ArrayDeque<TreeNode> q = new ArrayDeque<>(); q.add(root); int i = 1; while (!q.isEmpty() && i < values.length) { TreeNode node = q.remove(); if (i < values.length && values[i] != null) { node.left = new TreeNode(values[i]); q.add(node.left); } i++; if (i < values.length && values[i] != null) { node.right = new TreeNode(values[i]); q.add(node.right); } i++; } return root; }
  static TreeNode findTreeNode(TreeNode root, int value) { ArrayDeque<TreeNode> q = new ArrayDeque<>(); if (root != null) q.add(root); while (!q.isEmpty()) { TreeNode node = q.remove(); if (node.val == value) return node; if (node.left != null) q.add(node.left); if (node.right != null) q.add(node.right); } throw new IllegalArgumentException("Could not find tree node value."); }
  static Node randomList(int[][] items) { ArrayList<Node> nodes = new ArrayList<>(); for (int[] item : items) nodes.add(new Node(item[0])); originalRandomNodes = new HashSet<>(nodes); for (int i = 0; i < nodes.size(); i++) { nodes.get(i).next = i + 1 < nodes.size() ? nodes.get(i + 1) : null; if (items[i][1] >= 0) nodes.get(i).random = nodes.get(items[i][1]); } return nodes.isEmpty() ? null : nodes.get(0); }
  static String randomListJson(Node node) { ArrayList<Node> nodes = new ArrayList<>(); HashMap<Node, Integer> index = new HashMap<>(); HashSet<Node> seen = new HashSet<>(); for (Node cur = node; cur != null && !seen.contains(cur); cur = cur.next) { if (originalRandomNodes.contains(cur)) throw new IllegalArgumentException("Returned list must be a deep copy."); seen.add(cur); index.put(cur, nodes.size()); nodes.add(cur); } StringJoiner out = new StringJoiner(",", "[", "]"); for (Node cur : nodes) { if (cur.random != null && !index.containsKey(cur.random)) throw new IllegalArgumentException("Returned random pointer must stay inside the copied list."); out.add("[" + cur.val + "," + (cur.random == null ? "null" : index.get(cur.random)) + "]"); } return out.toString(); }
  static Node graphNode(int[][] adjacency) { if (adjacency.length == 0) { originalGraphNodes = new HashSet<>(); return null; } ArrayList<Node> nodes = new ArrayList<>(); for (int i = 0; i < adjacency.length; i++) nodes.add(new Node(i + 1)); originalGraphNodes = new HashSet<>(nodes); for (int i = 0; i < adjacency.length; i++) for (int value : adjacency[i]) nodes.get(i).neighbors.add(nodes.get(value - 1)); return nodes.get(0); }
  static String graphJson(Node node) { if (node == null) return "[]"; TreeMap<Integer, Node> byValue = new TreeMap<>(); ArrayDeque<Node> q = new ArrayDeque<>(); q.add(node); while (!q.isEmpty()) { Node cur = q.remove(); if (cur == null) continue; if (originalGraphNodes.contains(cur)) throw new IllegalArgumentException("Returned graph must be a deep copy."); if (byValue.containsKey(cur.val)) continue; byValue.put(cur.val, cur); for (Node next : cur.neighbors) q.add(next); } StringJoiner out = new StringJoiner(",", "[", "]"); for (Node cur : byValue.values()) { ArrayList<Integer> row = new ArrayList<>(); for (Node next : cur.neighbors) row.add(next.val); Collections.sort(row); out.add(json(row)); } return out.toString(); }
  static Node nextTree(Integer[] values) { if (values.length == 0 || values[0] == null) return null; Node root = new Node(values[0]); ArrayDeque<Node> q = new ArrayDeque<>(); q.add(root); int i = 1; while (!q.isEmpty() && i < values.length) { Node node = q.remove(); if (i < values.length && values[i] != null) { node.left = new Node(values[i]); q.add(node.left); } i++; if (i < values.length && values[i] != null) { node.right = new Node(values[i]); q.add(node.right); } i++; } return root; }
  static String nextTreeJson(Node root) { ArrayList<String> levels = new ArrayList<>(); ArrayDeque<Node> q = new ArrayDeque<>(); if (root != null) q.add(root); while (!q.isEmpty()) { int count = q.size(); ArrayList<Node> level = new ArrayList<>(); for (int i = 0; i < count; i++) { Node node = q.remove(); level.add(node); if (node.left != null) q.add(node.left); if (node.right != null) q.add(node.right); } ArrayList<Integer> row = new ArrayList<>(); for (int i = 0; i < level.size(); i++) { if (level.get(i).next != (i + 1 < level.size() ? level.get(i + 1) : null)) throw new IllegalArgumentException("next pointers do not match the tree level order."); row.add(level.get(i).val); } levels.add(json(row)); } return "[" + String.join(",", levels) + "]"; }
  static String quadTreeJson(Node root) { ArrayList<String> rows = new ArrayList<>(); quadTreeVisit(root, rows); return "[" + String.join(",", rows) + "]"; }
  static void quadTreeVisit(Node node, ArrayList<String> rows) { if (node == null) return; rows.add("[" + (node.isLeaf ? 1 : 0) + "," + (node.isLeaf ? (node.val == 0 ? 0 : 1) : 1) + "]"); if (!node.isLeaf) { quadTreeVisit(node.topLeft, rows); quadTreeVisit(node.topRight, rows); quadTreeVisit(node.bottomLeft, rows); quadTreeVisit(node.bottomRight, rows); } }
  public static void main(String[] args) {
    ArrayList<String> results = new ArrayList<>();
${spec.cases.map((testCase) => javaCase(spec, testCase)).join("\n")}
    System.out.print("{\\"results\\":[" + String.join(",", results) + "]}");
  }
}`;
}

function javaClassHarness(spec, candidateCode) {
  candidateCode = candidateCode.replace(new RegExp(`\\bpublic\\s+class\\s+${spec.className}\\b`, "g"), `class ${spec.className}`);
  return `import java.util.*;
class ListNode { int val; ListNode next; ListNode() { this(0, null); } ListNode(int val) { this(val, null); } ListNode(int val, ListNode next) { this.val = val; this.next = next; } }
class TreeNode { int val; TreeNode left; TreeNode right; TreeNode() { this(0, null, null); } TreeNode(int val) { this(val, null, null); } TreeNode(int val, TreeNode left, TreeNode right) { this.val = val; this.left = left; this.right = right; } }
${candidateCode}
class Main {
  static String json(String value) { StringBuilder out = new StringBuilder("\\""); for (char c : value.toCharArray()) { if (c == '\\\\' || c == '"') out.append('\\\\').append(c); else if (c < 0x20) out.append(String.format("\\\\u%04x", (int) c)); else out.append(c); } return out.append('"').toString(); }
  static String json(boolean value) { return value ? "true" : "false"; }
  static String json(int value) { return String.valueOf(value); }
  static String json(double value) { return Double.isFinite(value) ? String.valueOf(value) : "null"; }
  static String jsonAny(Object value) { if (value == null) return "null"; if (value instanceof String s) return json(s); if (value instanceof Integer i) return json(i); if (value instanceof Double d) return json(d); if (value instanceof Boolean b) return json(b); return json(String.valueOf(value)); }
  static TreeNode treeNode(Integer[] values) { if (values.length == 0 || values[0] == null) return null; TreeNode root = new TreeNode(values[0]); ArrayDeque<TreeNode> q = new ArrayDeque<>(); q.add(root); int i = 1; while (!q.isEmpty() && i < values.length) { TreeNode node = q.remove(); if (i < values.length && values[i] != null) { node.left = new TreeNode(values[i]); q.add(node.left); } i++; if (i < values.length && values[i] != null) { node.right = new TreeNode(values[i]); q.add(node.right); } i++; } return root; }
  public static void main(String[] args) {
    ArrayList<String> results = new ArrayList<>();
${spec.cases.map((testCase) => javaClassCase(spec, testCase)).join("\n")}
    System.out.print("{\\"results\\":[" + String.join(",", results) + "]}");
  }
}`;
}

function javaCase(spec, testCase) {
  const declarations = spec.paramTypes.map((type, paramIndex) => javaDeclaration(spec, testCase, type, paramIndex)).join("\n");
  const callArgs = callParamNames(spec).join(", ");
  const call = Number.isInteger(spec.outputPrefixParam)
    ? `int outputSize = solution.${spec.entry}(${callArgs});\n      ${javaPrefixGuard(spec, "outputSize")}\n      Object actual = ${javaPrefixExpression(spec, "outputSize")};`
    : spec.returnType === "void"
      ? `solution.${spec.entry}(${callArgs});\n      Object actual = ${javaActualExpression(spec)};`
      : `Object actual = solution.${spec.entry}(${callArgs});`;
  return `    try {
${declarations}
      Solution solution = new Solution();
      long start = System.nanoTime();
      ${call}
      double elapsed = (System.nanoTime() - start) / 1000000.0;
      results.add("{\\"actual\\":" + ${javaJsonExpression(spec, "actual")} + ",\\"timeMs\\":" + json(elapsed) + "}");
    } catch (Throwable error) {
      results.add("{\\"error\\":" + json(error.toString()) + ",\\"timeMs\\":0}");
    }`;
}

function javaClassCase(spec, testCase) {
  const [methods, argsList] = testCase.input;
  return `    try {
      long start = System.nanoTime();
      ${spec.className} instance = new ${spec.className}(${javaClassArgs(argsList[0], spec.constructorArgTypes).join(", ")});
      ArrayList<String> actual = new ArrayList<>();
      actual.add("null");
${methods.slice(1).map((method, index) => javaClassMethodCall(method, argsList[index + 1], testCase.expected[index + 1])).join("\n")}
      double elapsed = (System.nanoTime() - start) / 1000000.0;
      results.add("{\\"actual\\":[" + String.join(",", actual) + "],\\"timeMs\\":" + json(elapsed) + "}");
    } catch (Throwable error) {
      results.add("{\\"error\\":" + json(error.toString()) + ",\\"timeMs\\":0}");
    }`;
}

function javaClassMethodCall(method, args, expected) {
  const call = `instance.${method}(${(args || []).map((value) => javaClassLiteral(value)).join(", ")})`;
  return expected === null
    ? `      ${call};\n      actual.add("null");`
    : `      actual.add(jsonAny(${call}));`;
}

function javaClassArgs(args, argTypes) {
  return (args || []).map((value, index) => {
    if (argTypes?.[index] === "binaryTree") return treeNodeLiteral(value, "java");
    return javaClassLiteral(value);
  });
}

function javaClassLiteral(value) {
  if (typeof value === "string") return nativeLiteral(value, "string", "java");
  if (typeof value === "boolean") return nativeLiteral(value, "boolean", "java");
  if (Number.isInteger(value)) return nativeLiteral(value, "integer", "java");
  if (typeof value === "number") return nativeLiteral(value, "double", "java");
  if (Array.isArray(value)) return nativeLiteral(value, "integer[]", "java");
  if (value === null) return "null";
  throw new Error(`Unsupported Java class literal: ${JSON.stringify(value)}`);
}

function javaActualExpression(spec) {
  if (Number.isInteger(spec.outputParam)) return spec.paramNames[spec.outputParam];
  return "actual";
}

function javaPrefixExpression(spec, lengthName) {
  if (Number.isInteger(spec.outputPrefixParam)) {
    const name = spec.paramNames[spec.outputPrefixParam];
    return `Arrays.copyOf(${name}, ${lengthName})`;
  }
  return lengthName;
}

function javaPrefixGuard(spec, lengthName) {
  const name = spec.paramNames[spec.outputPrefixParam];
  return `if (${lengthName} < 0 || ${lengthName} > ${name}.length) throw new IllegalArgumentException("Return value must be an integer length for the kept prefix.");`;
}

function cppJsonExpression(spec, actualName) {
  if (spec.outputType === "randomList") return `randomListToJson(${actualName})`;
  if (spec.outputType === "graphNode") return `graphToJson(${actualName})`;
  if (spec.outputType === "nextTree") return `nextTreeToJson(${actualName})`;
  if (spec.outputType === "quadTree") return `quadTreeToJson(${actualName})`;
  return `toJson(${actualName})`;
}

function javaJsonExpression(spec, actualName) {
  if (spec.outputType === "randomList") return `randomListJson((Node) ${actualName})`;
  if (spec.outputType === "graphNode") return `graphJson((Node) ${actualName})`;
  if (spec.outputType === "nextTree") return `nextTreeJson((Node) ${actualName})`;
  if (spec.outputType === "quadTree") return `quadTreeJson((Node) ${actualName})`;
  return `jsonAny(${actualName})`;
}

function cppDeclaration(spec, testCase, type, index) {
  const name = spec.paramNames[index];
  const argType = spec.argTypes?.[index];
  const value = testCase.input[index];
  if (argType === "cyclePos") return `    int ${name} = ${nativeLiteral(value, "integer", "cpp")};`;
  if (argType === "linkedList") {
    const pos = Number.isInteger(spec.cyclePosParam) ? nativeLiteral(testCase.input[spec.cyclePosParam], "integer", "cpp") : "-1";
    return `    ListNode* ${name} = listNode(${nativeLiteral(value, "integer[]", "cpp")}, ${pos});`;
  }
  if (argType === "randomList") return `    Node* ${name} = randomList(${randomListLiteral(value, "cpp")});`;
  if (argType === "graphNode") return `    Node* ${name} = graphNode(${nativeLiteral(value, "integer[][]", "cpp")});`;
  if (argType === "binaryTree") return `    TreeNode* ${name} = ${treeNodeLiteral(value, "cpp")};`;
  if (argType === "nextTree") return `    Node* ${name} = nextTree(vector<optional<int>>{${(value || []).map((item) => item === null ? "nullopt" : String(item)).join(", ")}});`;
  if (argType === "treeNodeValue") {
    const root = spec.paramNames[spec.nodeRefRootParam ?? 0];
    return `    TreeNode* ${name} = findTreeNode(${root}, ${nativeLiteral(value, "integer", "cpp")});`;
  }
  return `    ${compilerType(type, "cpp")} ${name} = ${nativeLiteral(value, type, "cpp")};`;
}

function javaDeclaration(spec, testCase, type, index) {
  const name = spec.paramNames[index];
  const argType = spec.argTypes?.[index];
  const value = testCase.input[index];
  if (argType === "cyclePos") return `      int ${name} = ${nativeLiteral(value, "integer", "java")};`;
  if (argType === "linkedList") {
    const pos = Number.isInteger(spec.cyclePosParam) ? nativeLiteral(testCase.input[spec.cyclePosParam], "integer", "java") : "-1";
    return `      ListNode ${name} = listNode(${nativeLiteral(value, "integer[]", "java")}, ${pos});`;
  }
  if (argType === "randomList") return `      Node ${name} = randomList(${randomListLiteral(value, "java")});`;
  if (argType === "graphNode") return `      Node ${name} = graphNode(${nativeLiteral(value, "integer[][]", "java")});`;
  if (argType === "binaryTree") return `      TreeNode ${name} = ${treeNodeLiteral(value, "java")};`;
  if (argType === "nextTree") return `      Node ${name} = nextTree(new Integer[]{${(value || []).map((item) => item === null ? "null" : String(item)).join(", ")}});`;
  if (argType === "treeNodeValue") {
    const root = spec.paramNames[spec.nodeRefRootParam ?? 0];
    return `      TreeNode ${name} = findTreeNode(${root}, ${nativeLiteral(value, "integer", "java")});`;
  }
  return `      ${compilerType(type, "java")} ${name} = ${nativeLiteral(value, type, "java")};`;
}

function cDeclaration(spec, testCase, type, index) {
  const name = spec.paramNames[index];
  const argType = spec.argTypes?.[index];
  const value = testCase.input[index];
  if (argType === "cyclePos") return `    int ${name} = ${nativeLiteral(value, "integer", "c")};`;
  if (argType === "linkedList") {
    const array = cArrayDeclaration(`${name}Values`, value, "integer");
    const pos = Number.isInteger(spec.cyclePosParam) ? nativeLiteral(testCase.input[spec.cyclePosParam], "integer", "c") : "-1";
    return `${array}\n    struct ListNode* ${name} = listNodeCycle(${name}Values, ${name}ValuesSize, ${pos});\n    int ${name}Size = ${name}ValuesSize;`;
  }
  if (argType === "linkedListArray") return cLinkedListArrayDeclaration(name, value);
  if (argType === "binaryTree") return cTreeDeclaration(name, value, "treeNode");
  if (argType === "nextTree") return cTreeDeclaration(name, value, "nextTree", "struct Node*");
  if (argType === "treeNodeValue") {
    const root = spec.paramNames[spec.nodeRefRootParam ?? 0];
    return `    struct TreeNode* ${name} = findTreeNode(${root}, ${nativeLiteral(value, "integer", "c")});`;
  }
  if (argType === "randomList") return cRandomListDeclaration(name, value);
  if (argType === "graphNode") return `${cMatrixDeclaration(`${name}Adjacency`, value, "integer")}\n    struct Node* ${name} = graphNode(${name}Adjacency, ${name}AdjacencySize, ${name}AdjacencyColSize);`;
  if (type.endsWith("[][]") || /^list<list<.+>>$/.test(type)) return cMatrixDeclaration(name, value, nestedItemType(type));
  if (type.endsWith("[]") || /^list<.+>$/.test(type)) return cArrayDeclaration(name, value, nestedItemType(type));
  return `    ${compilerType(type, "c")} ${name} = ${nativeLiteral(value, type, "c")};`;
}

function callParamNames(spec) {
  return spec.paramNames.filter((_, index) => spec.argTypes?.[index] !== "cyclePos");
}

function arrayLiteral(value, itemType, language) {
  const values = value || [];
  if (language === "c") return `{${values.map((item) => nativeLiteral(item, itemType, language)).join(", ")}}`;
  if (language === "cpp") {
    if (itemType === "TreeNode") return `{${values.map((item) => nativeLiteral(item, itemType, language)).join(", ")}}`;
    if (itemType === "integer" && values.some((item) => item === null)) return `{${values.map((item) => item === null ? "nullopt" : String(item)).join(", ")}}`;
    return `{${values.map((item) => nativeLiteral(item, itemType, language)).join(", ")}}`;
  }
  const type = compilerType(`${itemType}[]`, language);
  return `new ${type}${javaArrayBody(values, itemType)}`;
}

function javaArrayBody(values, itemType) {
  return `{${(values || []).map((item) => {
    if (itemType === "integer" && item === null) return "null";
    return nativeLiteral(item, itemType, "java");
  }).join(", ")}}`;
}

function listLiteral(value, itemType, language) {
  const values = value || [];
  if (language === "c") return arrayLiteral(values, itemType, language);
  if (language === "cpp") return `{${values.map((item) => nativeLiteral(item, itemType, language)).join(", ")}}`;
  return `Arrays.asList(${values.map((item) => nativeLiteral(item, itemType, language)).join(", ")})`;
}

function randomListLiteral(value, language) {
  if (language === "cpp") {
    return `{${(value || []).map(([item, random]) => `{${item}, ${random === null ? "nullopt" : random}}`).join(", ")}}`;
  }
  const rows = (value || []).map(([item, random]) => [item, random ?? -1]);
  return nativeLiteral(rows, "integer[][]", language);
}

function helperCall(name, value, type, language, nullable = false) {
  if (nullable && value === null) return "null";
  return `${name}(${nativeLiteral(value || [], type, language)})`;
}

function treeNodeLiteral(value, language) {
  if (value === null) return "null";
  const values = value || [];
  if (language === "cpp") {
    return `treeNode(vector<optional<int>>{${values.map((item) => item === null ? "nullopt" : String(item)).join(", ")}})`;
  }
  return `treeNode(new Integer[]{${values.map((item) => item === null ? "null" : String(item)).join(", ")}})`;
}

function charLiteral(value) {
  return `'${String(value).replace(/['\\]/g, "\\$&")}'`;
}

function cNeedsSize(type) {
  return type.endsWith("[]") || type.endsWith("[][]") || /^list<.+>$/.test(type);
}

function cArrayDeclaration(name, value, itemType) {
  const values = value || [];
  const type = compilerType(itemType, "c");
  const literal = values.length ? arrayLiteral(values, itemType, "c") : "{0}";
  return `    ${type} ${name}[] = ${literal};\n    int ${name}Size = ${values.length};`;
}

function cMatrixDeclaration(name, value, itemType) {
  const rows = value || [];
  const rowDecls = rows.map((row, index) => cArrayDeclaration(`${name}Row${index}`, row, itemType)).join("\n");
  const rowNames = rows.map((_, index) => `${name}Row${index}`);
  const rowPointers = rowNames.length ? `{${rowNames.join(", ")}}` : "{NULL}";
  const colSizes = rows.length ? `{${rows.map((row) => (row || []).length).join(", ")}}` : "{0}";
  const pointerType = compilerType(`${itemType}[]`, "c");
  return `${rowDecls}
    ${pointerType} ${name}[] = ${rowPointers};
    int ${name}Size = ${rows.length};
    int ${name}ColSize[] = ${colSizes};`;
}

function cLinkedListArrayDeclaration(name, value) {
  const lists = value || [];
  const declarations = lists.map((items, index) => {
    const array = cArrayDeclaration(`${name}Values${index}`, items, "integer");
    return `${array}\n    struct ListNode* ${name}Node${index} = listNode(${name}Values${index}, ${name}Values${index}Size);`;
  }).join("\n");
  const nodeNames = lists.map((_, index) => `${name}Node${index}`);
  const pointers = nodeNames.length ? `{${nodeNames.join(", ")}}` : "{NULL}";
  return `${declarations}${declarations ? "\n" : ""}    struct ListNode* ${name}[] = ${pointers};\n    int ${name}Size = ${lists.length};`;
}

function cTreeDeclaration(name, value, helper, type = "struct TreeNode*") {
  const values = value || [];
  const encoded = values.map((item) => item === null ? 0 : item);
  const nulls = values.map((item) => item === null ? "true" : "false");
  return `    int ${name}Values[] = ${encoded.length ? `{${encoded.join(", ")}}` : "{0}"};
    bool ${name}Nulls[] = ${nulls.length ? `{${nulls.join(", ")}}` : "{true}"};
    int ${name}Size = ${values.length};
    ${type} ${name} = ${helper}(${name}Values, ${name}Nulls, ${name}Size);`;
}

function cRandomListDeclaration(name, value) {
  const rows = (value || []).map(([item, random]) => `{${item}, ${random ?? -1}}`);
  return `    int ${name}Values[][2] = ${rows.length ? `{${rows.join(", ")}}` : "{{0, -1}}"};
    int ${name}Size = ${(value || []).length};
    struct Node* ${name} = randomList(${name}Values, ${name}Size);`;
}

function nestedItemType(type) {
  const list = type.match(/^list<(.+)>$/);
  if (list) return nestedItemType(list[1]);
  while (type.endsWith("[]")) type = type.slice(0, -2);
  return type;
}

function boxedJavaType(type) {
  return { int: "Integer", double: "Double", boolean: "Boolean", char: "Character" }[type] || type;
}

function compilerText(value) {
  if (Array.isArray(value)) return stripAnsi(value.map((item) => item.text ?? item).join(""));
  return stripAnsi(value);
}
