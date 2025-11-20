# IO.puts and IO.inspect will be matched by pattern!
# Click diff tab to see rewrite.

defmodule MyApp.UserService do
  def create_user(params) do
    Logger.info("Creating user with params")
    Logger.debug(params, label: "User params")
    
    case validate_params(params) do
      {:ok, valid_params} ->
        Logger.info("Validation successful")
        create_user_record(valid_params)
      
      {:error, reason} ->
        Logger.debug(reason, label: "Validation error")
        {:error, reason}
    end
  end
  
  def process_batch(users) when is_list(users) do
    Logger.info("Processing batch of #{length(users)} users")
    
    users
    |> Enum.map(fn user ->
      Logger.debug(user.id, label: "Processing user")
      process_single_user(user)
    end)
  end
  
  defp validate_params(%{email: email} = params) do
    Logger.info("Validating email: #{email}")
    # validation logic here
    {:ok, params}
  end
end
