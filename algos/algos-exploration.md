# algos-exploration

[](/jiyuu/algos/001_two_sum.md)
[](/jiyuu/algos/002_remove_element.md)
[](/jiyuu/algos/003_move_zeros.md)
[](/jiyuu/algos/004_path_exists_in_grid.md)

---

## Unsolved

### Ex 00N: Remove Duplicates from Sorted Array

Practice Problem:

Given an integer array `nums` sorted in **non-decreasing order**, remove the
duplicates **in-place** such that each unique element appears only once. The
**relative order must stay the same**.

After removing duplicates:
- Let `k` be the count of unique elements.
- Your goal is to place the first `k` unique elements at the **front** of the array (`nums[0:k]`).
- The values after `k` do not matter.

Return `k`.

Rules:
* You must do this **without allocating extra space**.
* You may only modify the array **in-place**, using **O(1)** extra memory.

Custom Judge: Your code will be tested like this:
```
int[] nums = [...]; // sorted
int[] expected = [...]; // unique elements in sorted order

int k = removeDuplicates(nums);

assert k == expected.length;
for (int i = 0; i < k; i++) {
    assert nums[i] == expected[i];
}
```

If all assertions pass, your solution is accepted.

```
Example 1:
Input: nums = [1,1,2]
Output: 2, nums = [1,2,_]
Explanation: Your function should return k = 2, with the first two elements of
nums being 1 and 2 respectively.
It does not matter what you leave beyond the returned k (hence they are underscores).

Example 2:
Input: nums = [0,0,1,1,1,2,2,3,3,4]
Output: 5, nums = [0,1,2,3,4,_,_,_,_,_]
Explanation: Your function should return k = 5, with the first five elements of
nums being 0, 1, 2, 3, and 4 respectively.
It does not matter what you leave beyond the returned k (hence they are underscores).
```

Source Info:
- [Leetcode 26: Remove Duplicates from Sorted Array](https://leetcode.com/problems/remove-duplicates-from-sorted-array/description/)
- Topics: \[Array, Two Pointers\]

A: 
