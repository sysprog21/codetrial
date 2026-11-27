char* sumBitStrings(char* a, char* b) {
    int i = (int)strlen(a) - 1, j = (int)strlen(b) - 1;
    int carry = 0, length = 0;
    char* bits = malloc(strlen(a) + strlen(b) + 2);
    if (!bits) return NULL;
    while (i >= 0 || j >= 0 || carry) {
        int sum = carry;
        if (i >= 0) sum += a[i--] - '0';
        if (j >= 0) sum += b[j--] - '0';
        bits[length++] = '0' + sum % 2;
        carry = sum / 2;
    }
    bits[length] = '\0';
    for (int left = 0, right = length - 1; left < right; left++, right--) {
        char bit = bits[left];
        bits[left] = bits[right];
        bits[right] = bit;
    }
    return bits;
}
