// The task workspace's calculator, so arithmetic never needs another tab or
// app, and leaving the page ends an attempt. A small parser rather than
// `eval` or `Function`, which the page's content-security policy refuses and
// which would run whatever was typed.

/// Characters one expression may hold.
export const MAX_EXPRESSION_LENGTH = 200;

const RADIANS_PER_DEGREE = Math.PI / 180;

/// The remainder of floored division, so the result takes the divisor's sign
/// and `a == (a // b) * b + a % b` for integers. JavaScript's `%` truncates
/// instead. It adds the divisor only when the signs differ, because
/// `((a % b) + b) % b` would disturb `0.1 % 1`. An exact multiple gives 0,
/// because `atan2` reads the sign of -0. An infinite divisor keeps the plain
/// remainder, so a finite result stays finite. A dividend of the other sign
/// that is tiny beside the divisor rounds to the divisor itself: `-1e-20 % 3`
/// is 3, as with Python floats.
function flooredRemainder(dividend, divisor) {
  const remainder = dividend % divisor;
  if (remainder === 0) return 0;
  if (!Number.isFinite(divisor)) return remainder;
  return remainder < 0 !== divisor < 0 ? remainder + divisor : remainder;
}

/// A trigonometric function of an angle in the mode's unit. In degrees the
/// multiples of 90 are exact, so `cos(90)` is 0 and `tan(90)` is refused,
/// where radians would give 6e-17 and 1.6e16.
function trigonometric(name, exact, direct) {
  return (angle, argument) => {
    if (angle === "deg" && argument % 90 === 0) {
      const quarter = (((argument / 90) % 4) + 4) % 4;
      const value = exact[quarter];
      if (value === undefined)
        throw new Error(`${name} is undefined at ${argument} degrees`);
      return value;
    }
    const radians = angle === "deg" ? argument * RADIANS_PER_DEGREE : argument;
    return direct(radians);
  };
}

/// A function of one or more numbers: how many, and what it returns, given
/// the angle unit as its first argument.
const FUNCTIONS = {
  sqrt: [1, (_, x) => Math.sqrt(x)],
  cbrt: [1, (_, x) => Math.cbrt(x)],
  abs: [1, (_, x) => Math.abs(x)],
  exp: [1, (_, x) => Math.exp(x)],
  ln: [1, (_, x) => Math.log(x)],
  log: [1, (_, x) => Math.log10(x)],
  log2: [1, (_, x) => Math.log2(x)],
  floor: [1, (_, x) => Math.floor(x)],
  ceil: [1, (_, x) => Math.ceil(x)],
  round: [1, (_, x) => Math.round(x)],
  sin: [1, trigonometric("sin", [0, 1, 0, -1], Math.sin)],
  cos: [1, trigonometric("cos", [1, 0, -1, 0], Math.cos)],
  tan: [1, trigonometric("tan", [0, undefined, 0, undefined], Math.tan)],
  asin: [1, (angle, x) => fromRadians(angle, Math.asin(x))],
  acos: [1, (angle, x) => fromRadians(angle, Math.acos(x))],
  atan: [1, (angle, x) => fromRadians(angle, Math.atan(x))],
  atan2: [2, (angle, y, x) => fromRadians(angle, Math.atan2(y, x))],
  sinh: [1, (_, x) => Math.sinh(x)],
  cosh: [1, (_, x) => Math.cosh(x)],
  tanh: [1, (_, x) => Math.tanh(x)],
  pow: [2, (_, x, y) => x ** y],
  hypot: [2, (_, x, y) => Math.hypot(x, y)],
  min: [2, (_, x, y) => Math.min(x, y)],
  max: [2, (_, x, y) => Math.max(x, y)],
  deg: [1, (_, x) => x / RADIANS_PER_DEGREE],
  rad: [1, (_, x) => x * RADIANS_PER_DEGREE],
};
const CONSTANTS = { pi: Math.PI, e: Math.E };

/// An angle that came back from an inverse function, in the mode's unit.
function fromRadians(angle, radians) {
  return angle === "deg" ? radians / RADIANS_PER_DEGREE : radians;
}

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

/// A result as the calculator shows it. An integer up to 2^53, which a double
/// holds exactly, is shown whole, so `2^50` is not rounded; anything else keeps
/// fifteen significant digits, so `sin(30)` in degrees reads 0.5 and not
/// 0.49999999999999994.
export function formatResult(value) {
  if (Number.isInteger(value) && Math.abs(value) <= 2 ** 53)
    return String(value);
  return String(Number(value.toPrecision(15)));
}

/// The value of `text`, an arithmetic expression: numbers, `+ - * / // %`,
/// `^` or `**` for powers, parentheses, `pi`, `e`, and the functions above.
/// `angle` is the unit trigonometric functions take and return, "rad" or
/// "deg". Throws an Error that names what was wrong.
export function evaluate(text, { angle = "rad" } = {}) {
  if (angle !== "rad" && angle !== "deg")
    throw new Error("The angle unit is rad or deg");
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
  // The divisor of `/`, `//` or `%`. A zero divisor is refused here, where it
  // is read, because a later step can turn its NaN or Infinity back into a
  // finite number: `(5 % 0)^0` would be 1 and `min(1/0, 3)` would be 3.
  function divisor() {
    const value = unary();
    if (value === 0) throw new Error("Division by zero is undefined");
    return value;
  }
  function term() {
    let value = unary();
    for (;;) {
      if (take("*")) value *= unary();
      // Adding 0 turns a -0 quotient into 0, as `%` does, because `atan2`
      // reads the sign. A fractional operand can miss
      // `a == (a // b) * b + a % b`, because this rounds the quotient before
      // flooring it: `1 // 0.1` is 10.
      else if (take("//")) value = Math.floor(value / divisor()) + 0;
      else if (take("/")) value /= divisor();
      else if (take("%")) value = flooredRemainder(value, divisor());
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
        const [arity, apply] = FUNCTIONS[token.name];
        const values = [expression()];
        while (take(",")) values.push(expression());
        if (!take(")")) throw new Error("A parenthesis is not closed");
        if (values.length !== arity)
          throw new Error(
            `${token.name} takes ${arity} argument${arity === 1 ? "" : "s"}`,
          );
        return apply(angle, ...values);
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
