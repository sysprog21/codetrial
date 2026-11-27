static bool same(int** grid, int row, int col, int size) {
    int first = grid[row][col];
    for (int r = row; r < row + size; r++) {
        for (int c = col; c < col + size; c++) {
            if (grid[r][c] != first) return false;
        }
    }
    return true;
}

static struct Node* build(int** grid, int row, int col, int size) {
    struct Node* node = calloc(1, sizeof(*node));
    if (same(grid, row, col, size)) {
        node->val = grid[row][col];
        node->isLeaf = true;
        return node;
    }
    int half = size / 2;
    node->val = 1;
    node->isLeaf = false;
    node->topLeft = build(grid, row, col, half);
    node->topRight = build(grid, row, col + half, half);
    node->bottomLeft = build(grid, row + half, col, half);
    node->bottomRight = build(grid, row + half, col + half, half);
    return node;
}

struct Node* buildTileTree(int** grid, int gridSize, int* gridColSize) {
    (void)gridColSize;
    return build(grid, 0, 0, gridSize);
}
