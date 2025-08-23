# puts calls will be matched by pattern!
# click diff tab to see rewrite.

def process_data(data)
  logger.info("Processing data: #{data}")
  
  if data.nil?
    logger.info("Warning: data is nil")
    return false
  end
  
  result = data.transform(&:upcase)
  logger.info("Result: #{result}")
  
  # This string won't be matched
  message = "puts 'not me'"
  
  # Multiple argument puts
  logger.info("Status:", "completed")
  
  result
end

class DataProcessor
  def initialize(name)
    @name = name
    logger.info("Created processor: #{@name}")
  end
  
  def log_status
    logger.info("Processor #{@name} is ready")
  end
end