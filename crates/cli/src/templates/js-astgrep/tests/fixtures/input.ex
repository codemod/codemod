# IO.puts and IO.inspect will be matched by pattern!
# Click diff tab to see rewrite.

defmodule MyApp.UserService do
  def create_user(params) do
    IO.puts("Creating user with params")
    IO.inspect(params, label: "User params")
    
    case validate_params(params) do
      {:ok, valid_params} ->
        IO.puts("Validation successful")
        create_user_record(valid_params)
      
      {:error, reason} ->
        IO.inspect(reason, label: "Validation error")
        {:error, reason}
    end
  end
  
  def process_batch(users) when is_list(users) do
    IO.puts("Processing batch of #{length(users)} users")
    
    users
    |> Enum.map(fn user ->
      IO.inspect(user.id, label: "Processing user")
      process_single_user(user)
    end)
  end
  
  defp validate_params(%{email: email} = params) do
    IO.puts("Validating email: #{email}")
    # validation logic here
    {:ok, params}
  end
end
