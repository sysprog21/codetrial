def gameOfLife(board):
    rows, cols = len(board), len(board[0])
    before = [row[:] for row in board]
    for r in range(rows):
        for c in range(cols):
            live = sum(
                before[i][j]
                for i in range(max(0, r - 1), min(rows, r + 2))
                for j in range(max(0, c - 1), min(cols, c + 2))
                if (i, j) != (r, c)
            )
            board[r][c] = int(live == 3 or (before[r][c] and live == 2))
