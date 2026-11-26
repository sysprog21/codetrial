class Solution {
    bool same(vector<vector<int>>& grid, int row, int col, int size) {
        int first = grid[row][col];
        for (int r = row; r < row + size; r++) {
            for (int c = col; c < col + size; c++) {
                if (grid[r][c] != first) return false;
            }
        }
        return true;
    }
    Node* build(vector<vector<int>>& grid, int row, int col, int size) {
        if (same(grid, row, col, size)) return new Node(grid[row][col], true);
        int half = size / 2;
        return new Node(1, false,
            build(grid, row, col, half),
            build(grid, row, col + half, half),
            build(grid, row + half, col, half),
            build(grid, row + half, col + half, half));
    }
public:
    Node* buildTileTree(vector<vector<int>>& grid) {
        return build(grid, 0, 0, grid.size());
    }
};
