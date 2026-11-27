def findPeakElement(nums):
    # The judge compares exactly, so each case has one peak; a case with two
    # fails here rather than failing a correct candidate in the interview.
    def at(i):
        return nums[i] if 0 <= i < len(nums) else float("-inf")

    peaks = [i for i in range(len(nums)) if at(i - 1) < nums[i] > at(i + 1)]
    if len(peaks) != 1:
        raise ValueError(f"{len(peaks)} right answers: {peaks}")
    return peaks[0]
