#include "app/app.h"

#include "mathx/mathx.h"

namespace app {

int greet(int n) {
    return mathx::add(1, n);
}

}  // namespace app
