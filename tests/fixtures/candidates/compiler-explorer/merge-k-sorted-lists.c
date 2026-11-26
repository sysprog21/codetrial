struct ListNode* combineShardChains(struct ListNode** lists, int listsSize) {
    struct ListNode dummy = {0, NULL};
    struct ListNode* tail = &dummy;
    for (;;) {
        int best = -1;
        for (int i = 0; i < listsSize; i++) {
            if (lists[i] && (best < 0 || lists[i]->val < lists[best]->val)) best = i;
        }
        if (best < 0) break;
        tail->next = lists[best];
        tail = tail->next;
        lists[best] = lists[best]->next;
        tail->next = NULL;
    }
    return dummy.next;
}
