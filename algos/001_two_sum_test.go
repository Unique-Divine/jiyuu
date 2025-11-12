package algos

// Given an array of integers `nums` and an integer `target`, return indices of
// the two numbers such that they add up to `target`.
//
// You may assume that each input would have exactly one solution, and you may
// not use the same element twice.
//
// ```
// Example 1:
// Input: nums = [2,7,11,15], target = 9
// Output: [0,1]
// Explanation: Because nums[0] + nums[1] == 9, we return [0, 1].

// Example 2:
// Input: nums = [3,2,4], target = 6
// Output: [1,2]

// Example 3:
// Input: nums = [3,3], target = 6
// Output: [0,1]
// ```

// Inputs have exactly one solution -> there's guaranteed to be a correct answer.
// Cannot use same element twice -> [0, 0] is not a valid answer, even if it
// evaluates to the target.

func Sol1(nums []int, target int) [2]int {
	if len(nums) < 2 {
		return [2]int{-1, -1}
	}
	numToIdx := map[int]int{
		nums[0]: 0,
	}
	for i := 1; i < len(nums); i++ {
		num := nums[i]
		diffFromTarget := target - num
		if idxOther, ok := numToIdx[diffFromTarget]; ok {
			return [2]int{idxOther, i}
		}
		if _, hasNum := numToIdx[num]; hasNum {
			continue
		}
		numToIdx[num] = i
	}

	return [2]int{-1, -1}
}

func (s *S) Test1_TwoSum() {
	type testCase struct {
		nums   []int
		target int
		want   [2]int
	}

	for tcIdx, tc := range []testCase{
		{nums: []int{3, 3}, target: 6, want: [2]int{0, 1}},
		{nums: []int{3, 2, 4}, target: 6, want: [2]int{1, 2}},
		{nums: []int{2, 7, 11, 15}, target: 9, want: [2]int{0, 1}},
		// handles repeats
		{nums: []int{3, 2, 3}, target: 6, want: [2]int{0, 2}},
		// no solution cases
		{nums: []int{0}, target: 0, want: [2]int{-1, -1}},
		{nums: []int{}, target: 0, want: [2]int{-1, -1}},
		{nums: []int{3, 3}, target: 4, want: [2]int{-1, -1}},
	} {
		got := Sol1(tc.nums, tc.target)
		s.Require().Equalf(tc.want, got, "tc %d: %#v", tcIdx, tc)
	}

}
