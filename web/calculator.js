// The task workspace's calculator, so arithmetic never needs another tab or
// app, and leaving the page ends an attempt. A small parser rather than
// `eval` or `Function`, which the page's content-security policy refuses and
// which would run whatever was typed.

/// Characters one expression may hold.
export const MAX_EXPRESSION_LENGTH = 200;

const FUNCTIONS = {
  sqrt: Math.sqrt,
  abs: Math.abs,
  ln: Math.log,
  log: Math.log10,
  log2: Math.log2,
  floor: Math.floor,
  ceil: Math.ceil,
  round: Math.round,
};
const CONSTANTS = { pi: Math.PI, e: Math.E };

function tokenize(text) {
  const tokens = [];
  const pattern =
    /\s*(?:(\d+(?:\.\d*)?(?:e[+-]?\d+)?|\.\d+(?:e[+-]?\d+)?)|([a-z][a-z0-9]*)|(\*\*|\/\/|[-+*/%^(),]))/giy;
  let at = 0;
  while (at < text.length) {
    pattern.lastIndex = at;
    const match = pattern.exec(text);
    if (!match) {
      if (!text.slice(at).trim()) break;
      throw new Error(`Unexpected "${text.slice(at).trim()[0]}"`);
    }
    if (match[1] !== undefined) tokens.push({ number: Number(match[1]) });
    else if (match[2] !== undefined)
      tokens.push({ name: match[2].toLowerCase() });
    else tokens.push({ op: match[3] });
    at = pattern.lastIndex;
  }
  return tokens;
}

/// The value of `text`, an arithmetic expression: numbers, `+ - * / // %`,
/// `^` or `**` for powers, parentheses, `pi`, `e`, and the functions above.
/// Throws an Error that names what was wrong.
export function evaluate(text) {
  if (typeof text !== "string" || !text.trim())
    throw new Error("Enter an expression");
  if (text.length > MAX_EXPRESSION_LENGTH)
    throw new Error("The expression is too long");
  const tokens = tokenize(text);
  let at = 0;
  const peek = () => tokens[at];
  const take = (op) => {
    if (peek()?.op === op) {
      at += 1;
      return true;
    }
    return false;
  };
  function expression() {
    let value = term();
    for (;;) {
      if (take("+")) value += term();
      else if (take("-")) value -= term();
      else return value;
    }
  }
  function term() {
    let value = unary();
    for (;;) {
      if (take("*")) value *= unary();
      else if (take("//")) value = Math.floor(value / unary());
      else if (take("/")) value /= unary();
      else if (take("%")) value %= unary();
      else return value;
    }
  }
  function unary() {
    if (take("-")) return -unary();
    if (take("+")) return unary();
    return power();
  }
  function power() {
    const base = primary();
    // Right-associative, and binding tighter than unary minus on its left:
    // `-2^2` is -4, as in written mathematics.
    if (take("^") || take("**")) return base ** unary();
    return base;
  }
  function primary() {
    const token = peek();
    if (!token) throw new Error("The expression ends too early");
    at += 1;
    if (token.number !== undefined) return token.number;
    if (token.op === "(") {
      const value = expression();
      if (!take(")")) throw new Error("A parenthesis is not closed");
      return value;
    }
    if (token.name !== undefined) {
      if (Object.hasOwn(CONSTANTS, token.name)) return CONSTANTS[token.name];
      if (Object.hasOwn(FUNCTIONS, token.name)) {
        if (!take("(")) throw new Error(`${token.name} needs parentheses`);
        const value = expression();
        if (!take(")")) throw new Error("A parenthesis is not closed");
        return FUNCTIONS[token.name](value);
      }
      throw new Error(`Unknown name "${token.name}"`);
    }
    throw new Error(`Unexpected "${token.op}"`);
  }
  const value = expression();
  if (at < tokens.length)
    throw new Error(
      `Unexpected "${tokens[at].op ?? tokens[at].name ?? tokens[at].number}"`,
    );
  if (!Number.isFinite(value))
    throw new Error("The result is not a finite number");
  return value;
}
