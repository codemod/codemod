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
    Widget* widget = new Widget(42);
    int* numbers = new int[10];
    std::string* message = new std::string("Hello");
    
    // Usage patterns
    widget->getValue();
    numbers[0] = 5;
    *message = "Updated";
    
    // This won't be matched (already smart pointer)
    auto smart_widget = std::make_unique<Widget>(100);
    
    // Complex allocation
    Widget* complex = new Widget(widget->getValue() + 10);
    
    // Manual cleanup (will need to be removed)
    delete widget;
    delete[] numbers;
    delete message;
    delete complex;
    
    return 0;
}
