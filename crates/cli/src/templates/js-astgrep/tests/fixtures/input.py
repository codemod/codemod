# Example of Python 2-style exception handling
# This codemod will modernize exception syntax

# Old except syntax with comma
try:
    with open("file.txt", "r") as f:
        data = f.read()
except IOError as e:
    print("Error reading file: {}".format(e))

# Another example with variable
try:
    config = json.loads(data)
except ValueError as err:
    logging.error("Invalid JSON format: %s", err)

# Handling multiple exceptions (old style)
try:
    result = process_data(data)
except (KeyError, TypeError) as error:
    print("Processing failed: {}".format(error))

# Not everything needs updating
try:
    # This is already using modern syntax
    response = requests.get("https://api.example.com/data")
    response.raise_for_status()
except requests.RequestException as e:
    print(f"API request failed: {e}")
