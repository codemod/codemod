// console.log() will be matched by pattern!
// click diff tab to see rewrite.

function tryAstGrep() {
    console.log('matched in metavar!')
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
    console.debug('matched by YAML')
  }