int** addBusyBlock(int** intervals, int intervalsSize, int* intervalsColSize, int* newInterval, int newIntervalSize, int* returnSize, int** returnColumnSizes) {
    int** out = malloc(sizeof(int*) * (intervalsSize + 1));
    *returnColumnSizes = malloc(sizeof(int) * (intervalsSize + 1));
    int n = 0, i = 0, start = newInterval[0], end = newInterval[1];
    while (i < intervalsSize && intervals[i][1] < start) out[n++] = intervals[i++];
    for (; i < intervalsSize && intervals[i][0] <= end; i++) {
        if (intervals[i][0] < start) start = intervals[i][0];
        if (intervals[i][1] > end) end = intervals[i][1];
    }
    out[n] = malloc(2 * sizeof(int));
    out[n][0] = start;
    out[n++][1] = end;
    while (i < intervalsSize) out[n++] = intervals[i++];
    for (int k = 0; k < n; k++) (*returnColumnSizes)[k] = 2;
    *returnSize = n;
    return out;
}
