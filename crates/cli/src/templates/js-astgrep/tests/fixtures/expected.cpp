// Raw pointer allocations will be converted to smart pointers
// This demonstrates modernizing manual memory management to RAII

#include <memory>
#include <vector>
#include <string>

class Widget {
public:
    Widget(int value) : value_(value) {}
    int getValue() const { return value_; }
private:
    int value_;
};

int main() {
    // These raw pointer allocations will be transformed
    auto widget = std::make_unique<Widget>();
    std::vector<int> numbers(10);
    auto message = std::make_unique<std::string>();
    
    // Usage patterns
    widget->getValue();
    numbers[0] = 5;
    *message = "Updated";
    
    // This won't be matched (already smart pointer)
    auto smart_widget = std::make_unique<Widget>(100);
    
    // Complex allocation
    auto complex = std::make_unique<Widget>();
    
    // Manual cleanup (will need to be removed)
    // Automatic cleanup with smart pointers
    // Automatic cleanup with smart pointers
    // Automatic cleanup with smart pointers
    // Automatic cleanup with smart pointers
    
    return 0;
}
