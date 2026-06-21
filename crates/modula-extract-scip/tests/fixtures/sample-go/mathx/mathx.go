package mathx

func Double(n int) int {
	return n + n
}

func Add(a, b int) int {
	return Double(a) + b
}

type Calc struct{}

func (c Calc) Run() int {
	return Add(1, 2)
}
