class Solution:
    def sumBitStrings(self, a: str, b: str) -> str:
        def add(a, b, carry=0):
            if not a and not b:
                return "1" if carry else ""
            total = int(a[-1] if a else 0) + int(b[-1] if b else 0) + carry
            return add(a[:-1], b[:-1], total // 2) + str(total % 2)
        return add(a, b).lstrip("0") or "0"
