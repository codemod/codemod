const { Web3 } = require('web3');
const contractABI = require("../contract-abi.json");
const contractAddress = "0x4C4a07F737Bf57F6632B6CAB089B78f62385aCaE";
const web3 = new Web3("http://localhost:8545");

async function loadContract() {
  return new web3.eth.Contract(contractABI, contractAddress);
}

const contract = new web3.eth.Contract(ABI, ADDRESS);
contract.methods.myMethod().send({ from: accounts[0] });
