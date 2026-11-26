static void reverseRange(char* s, int left, int right) {
    while (left < right) {
        char temp = s[left];
        s[left++] = s[right];
        s[right--] = temp;
    }
}
char* restoreCaptionOrder(char* s) {
    int read = 0, write = 0;
    while (s[read]) {
        while (s[read] == ' ') read++;
        if (!s[read]) break;
        if (write) s[write++] = ' ';
        while (s[read] && s[read] != ' ') s[write++] = s[read++];
    }
    s[write] = '\0';
    reverseRange(s, 0, write - 1);
    int start = 0;
    for (int i = 0; i <= write; i++) {
        if (s[i] == ' ' || s[i] == '\0') {
            reverseRange(s, start, i - 1);
            start = i + 1;
        }
    }
    return s;
}
