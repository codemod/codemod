const provider = new ethers.providers.JsonRpcProvider();
const balance = await provider.getBalance(address);
const total = balance.add(ethers.BigNumber.from("1000"));
console.log(total.toString());
