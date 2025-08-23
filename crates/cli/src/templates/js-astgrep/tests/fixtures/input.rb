# puts calls will be matched by pattern!
# click diff tab to see rewrite.

def process_data(data)
    puts "Processing data: #{data}"
    
    if data.nil?
      puts "Warning: data is nil"
      return false
    end
    
    result = data.transform(&:upcase)
    puts "Result: #{result}"
    
    # This string won't be matched
    message = "puts 'not me'"
    
    # Multiple argument puts
    puts "Status:", "completed"
    
    result
  end
  
  class DataProcessor
    def initialize(name)
      @name = name
      puts "Created processor: #{@name}"
    end
    
    def log_status
      puts "Processor #{@name} is ready"
    end
  end