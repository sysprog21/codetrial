def evalRPN(tokens):
    stack = []
    for token in tokens:
        if token in "+-*/":
            b, a = stack.pop(), stack.pop()
            if token == "+":
                stack.append(a + b)
            elif token == "-":
                stack.append(a - b)
            elif token == "*":
                stack.append(a * b)
            else:
                # Division truncates toward zero.
                stack.append(abs(a) // abs(b) * (1 if (a < 0) == (b < 0) else -1))
        else:
            stack.append(int(token))
    return stack[0]
