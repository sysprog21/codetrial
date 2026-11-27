struct Node* snapshotOutline(struct Node* head) {
    while (head && head->next) head = head->next;
    return head;
}
