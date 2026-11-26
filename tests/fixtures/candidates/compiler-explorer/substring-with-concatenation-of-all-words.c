int* findChunkBlockOffsets(char* s, char** words, int wordsSize, int* returnSize) {
    int length = strlen(s), width = strlen(words[0]);
    int span = width * wordsSize;
    int* out = malloc(sizeof(int) * (length + 1));
    bool* used = malloc(sizeof(bool) * wordsSize);
    *returnSize = 0;
    for (int start = 0; start + span <= length; start++) {
        memset(used, 0, sizeof(bool) * wordsSize);
        int matched = 0;
        for (int chunk = 0; chunk < wordsSize; chunk++) {
            const char* piece = s + start + chunk * width;
            int found = -1;
            for (int w = 0; w < wordsSize && found < 0; w++)
                if (!used[w] && strncmp(piece, words[w], width) == 0) found = w;
            if (found < 0) break;
            used[found] = true;
            matched++;
        }
        if (matched == wordsSize) out[(*returnSize)++] = start;
    }
    free(used);
    return out;
}
