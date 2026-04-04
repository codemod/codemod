from brownie import accounts, network, Token
def main():
    account = accounts[0]
    token = Token.deploy("MyToken", "MTK", 18, 1e21, {"from": account})
    print(f"Deployed at {token.address}")
