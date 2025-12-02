# algos-exploration

### Ex 001: Two Sum

Practice Problem:

Given an array of integers `nums` and an integer `target`, return indices of
the two numbers such that they add up to `target`.

You may assume that each input would have exactly one solution, and you may
not use the same element twice.

```
Example 1:
Input: nums = [2,7,11,15], target = 9
Output: [0,1]
Explanation: Because nums[0] + nums[1] == 9, we return [0, 1].

Example 2:
Input: nums = [3,2,4], target = 6
Output: [1,2]

Example 3:
Input: nums = [3,3], target = 6
Output: [0,1]
```

Source Info:
- This is [Leetcode 1: Two Sum](https://leetcode.com/problems/two-sum/)

Q: Describe the solution in words.

- Inputs have exactly one solution -> there's guaranteed to be a correct answer.
- Cannot use same element twice -> [0, 0] is not a valid answer, even if it
evaluates to the target.

- .x
- .x 
- .x

### Ex 002: Remove Element

Practice Problem:

Given an integer array `nums` and an integer `val`, remove all occurrences of
`val` in `nums` in-place. The order of the elements may be changed. Then return
the number of elements in nums which are not equal to `val`.

Consider the number of elements in nums which are not equal to `val` be `k`, to
get accepted, you need to do the following things:
- Change the array `nums` such that the first `k` elements of `nums` contain the
  elements which are not equal to `val`. The remaining elements of `nums` are not
  important as well as the size of `nums`.
- Return k.

Custom Judge:
The judge will test your solution with the following code:
```
int[] nums = [...]; // Input array
int val = ...; // Value to remove
int[] expectedNums = [...]; // The expected answer with correct length.
                            // It is sorted with no values equaling val.

int k = removeElement(nums, val); // Calls your implementation

assert k == expectedNums.length;
sort(nums, 0, k); // Sort the first k elements of nums
for (int i = 0; i < actualLength; i++) {
    assert nums[i] == expectedNums[i];
}
```
If all assertions pass, then your solution will be accepted.

Source Info:
- [Leetcode 27: Remove Element](https://leetcode.com/problems/remove-element/description/)
- Topics: \[Array, Two Pointers\]

A: 
Impl in 002_remove_element_test.go

### Ex 003: Move Zeros

Practice Problem:

Given an integer array nums, move all 0's to the end of it while maintaining the relative order of the non-zero elements.

Note that you must do this in-place without making a copy of the array.

```
Example 1:
Input: nums = [0,1,0,3,12]
Output: [1,3,12,0,0]

Example 2:
Input: nums = [0]
Output: [0]
```

Source Info:
- [Leetcode 283: Move Zeros](https://leetcode.com/problems/move-zeroes/)
- Topics: \[Array, Two Pointers\]

A: 
Impl in 003_move_zeros_test.go

### Ex 004: Remove Duplicates from Sorted Array

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
