class EventQueue {
    vector<int> values;
    int head = 0;
    int tail = 0;
    int count = 0;
public:
    EventQueue(int capacity) : values(capacity) {}
    bool push(int value) {
        if (count == (int)values.size()) return false;
        values[tail] = value;
        tail = (tail + 1) % values.size();
        count++;
        return true;
    }
    int pop() {
        if (count == 0) return -1;
        int value = values[head];
        head = (head + 1) % values.size();
        count--;
        return value;
    }
    int front() { return count == 0 ? -1 : values[head]; }
    int size() { return count; }
};
