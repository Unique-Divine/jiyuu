package algos

// Given an integer array `nums` and an integer `val`, remove all occurrences of
// `val` in `nums` in-place. The order of the elements may be changed. Then return
// the number of elements in nums which are not equal to `val`.
//
// Consider the number of elements in nums which are not equal to `val` be `k`, to
// get accepted, you need to do the following things:
// - Change the array `nums` such that the first `k` elements of `nums` contain the
//   elements which are not equal to `val`. The remaining elements of `nums` are not
//   important as well as the size of `nums`.
// - Return k.
//
// Custom Judge:
// The judge will test your solution with the following code:
// ```
// int[] nums = [...]; // Input array
// int val = ...; // Value to remove
// int[] expectedNums = [...]; // The expected answer with correct length.
//                             // It is sorted with no values equaling val.
//
// int k = removeElement(nums, val); // Calls your implementation
//
// assert k == expectedNums.length;
// sort(nums, 0, k); // Sort the first k elements of nums
// for (int i = 0; i < actualLength; i++) {
//     assert nums[i] == expectedNums[i];
// }
// ```
// If all assertions pass, then your solution will be accepted.

// Sol2: Pushes all "val" elements to the back of "nums".
//
// Args:
//   - nums: A slice we modify inplace to push elements equal to "val" to the
//     back.
//   - val: Target value that we want pushed to the end of the slice.
//
// Returns:
//   - k: Count of non-"val" elements in nums.
func Sol2(nums []int, val int) int {
	var (
		k        int = 0 // Count of non-val elements
		writeIdx int = 0 // Idx for the earliest spot we can place
	)

	for idx, num := range nums {
		if num == val {
			continue
		}
		nums[idx] = nums[writeIdx]
		nums[writeIdx] = num
		k++
		writeIdx++
	}

	return k
}

func (s *S) TestSol2() {
	for tcIdx, tc := range []struct {
		nums     []int
		val      int
		wantK    int
		wantNums []int
	}{
		{
			nums:     []int{0, 1, 0},
			val:      1,
			wantK:    2,
			wantNums: []int{0, 0, 1},
		},
		{
			nums:     []int{1, 0, 1},
			val:      1,
			wantK:    1,
			wantNums: []int{0, 1, 1},
		},
		{
			nums:     []int{1, 0, 0},
			val:      1,
			wantK:    2,
			wantNums: []int{0, 0, 1},
		},
		{
			nums:     []int{0, 1, 2, 2, 3, 0, 4, 2},
			val:      2,
			wantK:    5,
			wantNums: []int{0, 1, 3, 0, 4},
		},
		{
			nums:     []int{3, 2, 2, 3},
			val:      3,
			wantK:    2,
			wantNums: []int{2, 2},
		},
	} {
		s.T().Logf("tc %d: %#v", tcIdx, tc)
		nums := tc.nums
		gotK := Sol2(nums, tc.val)
		s.Equal(tc.wantK, gotK)
		if tc.wantK > 0 && tc.wantK < len(nums) {
			s.Equal(tc.wantNums[:tc.wantK], nums[:tc.wantK], "expect equal front")
			for _, num := range nums[tc.wantK:] {
				s.Require().Equal(tc.val, num, "expect equal back")
			}
		}

	}

}
