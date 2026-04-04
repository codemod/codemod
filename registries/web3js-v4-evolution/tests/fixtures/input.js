const web3 = new Web3("http://localhost:8545");
const contract = new web3.eth.Contract(ABI, ADDRESS);
contract.methods.myMethod().send({ from: accounts[0] });
