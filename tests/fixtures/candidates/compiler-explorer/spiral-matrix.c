int* clockwiseScanOrder(int** matrix, int matrixSize, int* matrixColSize, int* returnSize) {
    int rows = matrixSize, cols = matrixSize ? matrixColSize[0] : 0;
    int* out = malloc(sizeof(int) * (rows * cols + 1));
    int top = 0, bottom = rows - 1, left = 0, right = cols - 1;
    *returnSize = 0;
    while (top <= bottom && left <= right) {
        for (int j = left; j <= right; j++) out[(*returnSize)++] = matrix[top][j];
        for (int i = top + 1; i <= bottom; i++) out[(*returnSize)++] = matrix[i][right];
        if (top < bottom)
            for (int j = right - 1; j >= left; j--) out[(*returnSize)++] = matrix[bottom][j];
        if (left < right)
            for (int i = bottom - 1; i > top; i--) out[(*returnSize)++] = matrix[i][left];
        top++, bottom--, left++, right--;
    }
    return out;
}
