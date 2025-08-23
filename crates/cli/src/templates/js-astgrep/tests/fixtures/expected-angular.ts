<!-- Array-based ngClass with ternary expressions -->
<div [class]="`${[isPrimary ? 'primary' : ''} ${isBold ? 'bold' : ''} ${isLarge ? 'large' : '']}`.trim()">
  Primary Button
</div>

<!-- Another array-based ngClass example -->
<button [class]="`${[isActive ? 'active' : ''} ${isDisabled ? 'disabled' : '']}`.trim()">
  Click Me
</button>

<!-- More complex ngClass arrays with nested conditions -->
<span [class]="`${[
  isImportant ? 'important' : ''} ${isHighlighted ? 'highlight' : ''} ${isError ? 'error' : isWarning ? 'warning' : ''}`.trim()">Warning Message</span>

<!-- Already using class binding with template strings -->
<div [class]="`${{isBold ? 'bold' : ''}`.trim()}`.trim()">
  Already Optimized
</div>
