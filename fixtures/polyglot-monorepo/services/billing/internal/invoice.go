package invoice

// Total sums line amounts in cents.
func Total(lines []int64) int64 {
	var t int64
	for _, l := range lines {
		t += l
	}
	return t
}
