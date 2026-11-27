class Solution {
public:
    vector<vector<int>> addBusyBlock(vector<vector<int>>& intervals, vector<int>& newInterval) {
        vector<vector<int>> out;
        size_t i = 0;
        int start = newInterval[0], end = newInterval[1];
        while (i < intervals.size() && intervals[i][1] < start) out.push_back(intervals[i++]);
        for (; i < intervals.size() && intervals[i][0] <= end; i++) {
            start = min(start, intervals[i][0]);
            end = max(end, intervals[i][1]);
        }
        out.push_back({start, end});
        while (i < intervals.size()) out.push_back(intervals[i++]);
        return out;
    }
};
