#include <string>
#include "helper.h"

class Widget : public BaseWidget {
public:
    std::string render() {
        return helper_help(name_);
    }
private:
    std::string name_;
};

int main() {
    Widget w;
    w.render();
    return 0;
}
