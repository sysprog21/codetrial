class ThumbnailStore {
    int capacity;
    list<pair<int, int>> order;
    unordered_map<int, list<pair<int, int>>::iterator> byKey;
public:
    ThumbnailStore(int capacity) : capacity(capacity) {}
    int get(int key) {
        auto found = byKey.find(key);
        if (found == byKey.end()) return -1;
        order.splice(order.begin(), order, found->second);
        return found->second->second;
    }
    void put(int key, int value) {
        auto found = byKey.find(key);
        if (found != byKey.end()) {
            found->second->second = value;
            order.splice(order.begin(), order, found->second);
            return;
        }
        order.push_front({key, value});
        byKey[key] = order.begin();
        if ((int)order.size() > capacity) {
            byKey.erase(order.back().first);
            order.pop_back();
        }
    }
};
