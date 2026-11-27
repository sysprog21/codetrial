class Solution {
    public int[][] addBusyBlock(int[][] intervals, int[] newInterval) {
        int[][] out = new int[intervals.length + 1][];
        int n = 0, i = 0, start = newInterval[0], end = newInterval[1];
        while (i < intervals.length && intervals[i][1] < start) out[n++] = intervals[i++];
        for (; i < intervals.length && intervals[i][0] <= end; i++) {
            start = Math.min(start, intervals[i][0]);
            end = Math.max(end, intervals[i][1]);
        }
        out[n++] = new int[] {start, end};
        while (i < intervals.length) out[n++] = intervals[i++];
        int[][] result = new int[n][];
        for (int k = 0; k < n; k++) result[k] = out[k];
        return result;
    }
}
