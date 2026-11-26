static int compareCharacters(const void* a, const void* b) {
    return *(const unsigned char*)a - *(const unsigned char*)b;
}
bool isRearrangedCode(char* s, char* t) {
    size_t length = strlen(s);
    if (length != strlen(t)) return false;
    qsort(s, length, sizeof(*s), compareCharacters);
    qsort(t, length, sizeof(*t), compareCharacters);
    return strcmp(s, t) == 0;
}
