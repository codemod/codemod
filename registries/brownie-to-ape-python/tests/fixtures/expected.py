from ape import accounts, project
def main():
    account = accounts[0]
    token = account.deploy(project.Token, "MyToken", "MTK", 18, 1e21)
    print(f"Deployed at {token.address}")
