#include "mathx/mathx.h"

namespace mathx {

int doubleValue(int n) {
    return n + n;
}

int add(int a, int b) {
    return doubleValue(a) + b;
}

int Calc::run() {
    return add(1, 2);
}

}  // namespace mathx
