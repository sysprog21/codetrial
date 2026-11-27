class BidLedger {
    vector<int> values;
    vector<int> minimums;
public:
    BidLedger() {}
    void push(int value) {
        values.push_back(value);
        minimums.push_back(minimums.empty() ? value : min(value, minimums.back()));
    }
    void pop() {
        values.pop_back();
        minimums.pop_back();
    }
    int top() { return values.back(); }
    int getMin() { return minimums.back(); }
};
