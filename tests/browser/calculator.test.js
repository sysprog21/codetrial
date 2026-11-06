import test from "node:test";
import assert from "node:assert/strict";
import {
  evaluate,
  formatResult,
  MAX_EXPRESSION_LENGTH,
} from "../../web/calculator.js";

test("arithmetic follows ordinary precedence and associativity", () => {
  for (const [text, value] of [
    ["1 + 2 * 3", 7],
    ["(1 + 2) * 3", 9],
    ["2 ^ 3 ^ 2", 512],
    ["2 ** 10", 1024],
    ["-2 ^ 2", -4],
    ["2 ^ -1", 0.5],
    ["17 // 5", 3],
    ["17 % 5", 2],
    ["1e3 / 4", 250],
    [".5 + .25", 0.75],
    ["sqrt(16) + abs(-3)", 7],
    ["log(1000)", 3],
    ["round(pi * 100) / 100", 3.14],
  ])
    assert.equal(evaluate(text), value, text);
});

test("% is the remainder of the floored division // performs", () => {
  for (const [text, value] of [
    ["-7 // 3", -3],
    ["-7 % 3", 2],
    ["7 // -3", -3],
    ["7 % -3", -2],
    ["-7 // -3", 2],
    ["-7 % -3", -1],
    ["-7.5 % 2", 0.5],
    ["(-7 // 3) * 3 + (-7 % 3)", -7],
  ])
    assert.equal(evaluate(text), value, text);
  for (const a of [-7, -6, -1, 0, 1, 6, 7])
    for (const b of [-3, -2, 2, 3]) {
      const text = `(${a} // ${b}) * ${b} + (${a} % ${b})`;
      assert.equal(evaluate(text), a, text);
    }
});

test("neither // nor % leaves a negative zero for atan2 to read", () => {
  assert.equal(evaluate("-6 % 3"), 0);
  assert.equal(evaluate("-6 % -3"), 0);
  assert.equal(evaluate("-0 % 3"), 0);
  assert.equal(evaluate("atan2(-6 % 3, -1)"), Math.PI);
  assert.equal(evaluate("0 // -3"), 0);
  assert.equal(evaluate("-1 // 10^400"), 0);
  assert.equal(evaluate("atan2(0 // -3, -1)"), Math.PI);
});

test("// still rounds the quotient before flooring it", () => {
  for (const [text, value] of [
    ["1 // 0.1", 10],
    ["-9.3 // 3.1", -3],
  ])
    assert.equal(evaluate(text), value, text);
});

test("% keeps a finite result under an infinite divisor", () => {
  for (const [text, value] of [
    ["-1 % 10^400", -1],
    ["1 % -1e999", 1],
    ["5 % 10^400", 5],
    ["-5 % -1e999", -5],
  ])
    assert.equal(evaluate(text), value, text);
});

test("a tiny remainder of the other sign rounds to the divisor", () => {
  for (const [text, value] of [
    ["-1e-20 % 3", 3],
    ["1e-20 % -3", -3],
  ])
    assert.equal(evaluate(text), value, text);
});

test("% leaves a non-negative dividend and divisor alone", () => {
  for (const [text, value] of [
    ["7 % 3", 1],
    ["6 % 3", 0],
    ["0.1 % 1", 0.1],
    ["7.5 % 2", 1.5],
  ])
    assert.equal(evaluate(text), value, text);
});

test("a zero divisor that leaves the result not finite is refused", () => {
  for (const text of ["5 % 0", "-5 % 0", "0 % 0", "0 // 0"])
    assert.throws(
      () => evaluate(text),
      { message: "The result is not a finite number" },
      text,
    );
});

test("bad input names its problem and never runs code", () => {
  for (const [text, message] of [
    ["", "Enter an expression"],
    ["1 +", "The expression ends too early"],
    ["(1 + 2", "A parenthesis is not closed"],
    ["alert(1)", 'Unknown name "alert"'],
    ["1 / 0", "The result is not a finite number"],
    ["sqrt 4", "sqrt needs parentheses"],
    ["2 $ 3", 'Unexpected "$"'],
    ["constructor", 'Unknown name "constructor"'],
    ["1".repeat(MAX_EXPRESSION_LENGTH + 1), "The expression is too long"],
  ])
    assert.throws(() => evaluate(text), { message }, text);
  // Nothing typed is ever run as code: an expression that would change state
  // if it were is refused, and the state is untouched.
  globalThis.calculatorProbe = 0;
  for (const text of ["calculatorProbe = 1", "calculatorProbe++"])
    assert.throws(() => evaluate(text), Error, text);
  assert.equal(globalThis.calculatorProbe, 0);
  delete globalThis.calculatorProbe;
});

test("trigonometric functions take radians or degrees", () => {
  const close = (text, expected, options) =>
    assert.ok(
      Math.abs(evaluate(text, options) - expected) < 1e-12,
      `${text} ${options?.angle ?? "rad"}`,
    );
  close("sin(pi / 6)", 0.5);
  close("cos(pi / 3)", 0.5);
  close("tan(pi / 4)", 1);
  close("asin(1)", Math.PI / 2);
  close("atan2(1, -1)", (3 * Math.PI) / 4);
  close("sin(30)", 0.5, { angle: "deg" });
  close("tan(45)", 1, { angle: "deg" });
  close("acos(0.5)", 60, { angle: "deg" });
  close("atan2(1, 1)", 45, { angle: "deg" });
  close("sinh(1) + cosh(1)", Math.E);
  close("tanh(0)", 0);
});

test("multiples of a quarter turn are exact, and tangent refuses its poles", () => {
  const degrees = { angle: "deg" };
  for (const [text, value] of [
    ["sin(90)", 1],
    ["sin(180)", 0],
    ["sin(-90)", -1],
    ["sin(450)", 1],
    ["cos(90)", 0],
    ["cos(180)", -1],
    ["tan(180)", 0],
  ])
    assert.equal(evaluate(text, degrees), value, text);
  assert.throws(() => evaluate("tan(90)", degrees), {
    message: "tan is undefined at 90 degrees",
  });
  assert.throws(() => evaluate("tan(-270)", degrees), /undefined/);
  // Radians retain floating-point residuals rather than erasing small values.
  assert.equal(evaluate("sin(pi)"), Math.sin(Math.PI));
  assert.equal(evaluate("cos(pi / 2)"), Math.cos(Math.PI / 2));
  assert.equal(evaluate("sin(1e-20)"), 1e-20);
});

test("large angles retain nonzero trigonometric results", () => {
  for (const [text, expected] of [
    ["sin(1e15)", 0.8582727931702359],
    ["cos(1e15)", -0.5131937377869703],
    ["tan(1e15)", -1.672414782127583],
    ["sin(-1e15)", -0.8582727931702359],
    ["sin(3.141592653589794)", -7.657137397853899e-16],
  ])
    assert.ok(Math.abs((evaluate(text) - expected) / expected) < 1e-14, text);
});

test("exponential, root and multi-argument functions", () => {
  for (const [text, value] of [
    ["exp(0)", 1],
    ["exp(1)", Math.E],
    ["e ^ 2", Math.E ** 2],
    ["cbrt(27)", 3],
    ["pow(2, 10)", 1024],
    ["hypot(3, 4)", 5],
    ["min(2, 7) + max(2, 7)", 9],
    ["deg(pi)", 180],
    ["rad(180)", Math.PI],
    ["ln(exp(3))", 3],
  ])
    assert.equal(evaluate(text), value, text);
});

test("a function is called with the arguments it takes", () => {
  for (const [text, message] of [
    ["sin(1, 2)", "sin takes 1 argument"],
    ["atan2(1)", "atan2 takes 2 arguments"],
    ["pow(2,)", 'Unexpected ")"'],
    ["sin()", 'Unexpected ")"'],
  ])
    assert.throws(() => evaluate(text), { message }, text);
  assert.throws(() => evaluate("sin(1)", { angle: "gradians" }), {
    message: "The angle unit is rad or deg",
  });
});

test("results show twelve significant digits", () => {
  assert.equal(formatResult(evaluate("sin(30)", { angle: "deg" })), "0.5");
  assert.equal(formatResult(evaluate("acos(0.5)", { angle: "deg" })), "60");
  assert.equal(formatResult(evaluate("sin(pi / 6)")), "0.5");
  assert.equal(formatResult(1 / 3), "0.333333333333333");
  assert.equal(formatResult(2 ** 70), "1.18059162071741e+21");
  assert.equal(formatResult(1e-20), "1e-20");
  // An integer a double holds exactly is never rounded.
  assert.equal(formatResult(evaluate("2 ^ 50")), "1125899906842624");
  assert.equal(
    formatResult(evaluate("123456789012345 + 1")),
    "123456789012346",
  );
  assert.equal(formatResult(2 ** 53), "9007199254740992");
  assert.equal(formatResult(-(2 ** 53 - 1)), "-9007199254740991");
  assert.equal(formatResult(2 ** 54), "18014398509482000");
  assert.equal(formatResult(-0), "0");
});
