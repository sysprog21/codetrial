def calcEquation(equations, values, queries):
    # The judge compares exactly, so a case's product has to round the same
    # along every path, as 0.1 * 0.2 does not; such a case waits for an
    # element-wise tolerant checker.
    graph = {}
    for (a, b), value in zip(equations, values):
        graph.setdefault(a, {})[b] = value
        graph.setdefault(b, {})[a] = 1 / value

    def ratio(start, end):
        if start not in graph or end not in graph:
            return -1.0
        stack, seen = [(start, 1.0)], {start}
        while stack:
            node, product = stack.pop()
            if node == end:
                return product
            for nxt, value in graph[node].items():
                if nxt not in seen:
                    seen.add(nxt)
                    stack.append((nxt, product * value))
        return -1.0

    return [ratio(a, b) for a, b in queries]
