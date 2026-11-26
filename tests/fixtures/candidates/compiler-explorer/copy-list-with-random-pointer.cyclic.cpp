class Solution {
public:
    Node* snapshotOutline(Node* head) {
        if (!head) return nullptr;
        unordered_map<Node*, Node*> copies;
        for (Node* p = head; p; p = p->next) copies[p] = new Node(p->val);
        for (Node* p = head; p; p = p->next) {
            copies[p]->next = p->next ? copies[p->next] : copies[head];
            copies[p]->random = p->random ? copies[p->random] : nullptr;
        }
        return copies[head];
    }
};
