class Counter:
    def __init__(self):
        self.value = 0
    
    def increment(self):
        self.value += 1

c1 = Counter()
c2 = Counter()
print(isinstance(c1, Counter))



