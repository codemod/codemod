const provider = new ethers.JsonRpcProvider();
const balance = await provider.getBalance(address);
const total = balance + BigInt("1000");
console.log(total.toString());
