// console.log() will be matched by pattern!
// click diff tab to see rewrite.

function tryAstGrep() {
  logger.log('matched in metavar!')
}

const multiLineExpression =
  console
   .log({
     also: 'matched',
     multi: 'line',
     expression: 'console.log("you can\'t trick me!")',
   })

if (true) {
  const notThis = 'console.log("not me")'
} else {
  logger.log('matched by YAML')
}