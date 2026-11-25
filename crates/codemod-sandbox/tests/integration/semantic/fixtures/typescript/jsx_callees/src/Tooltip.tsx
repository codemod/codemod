export function Tooltip({ content, children }) {
  return (
    <span className="tooltip">
      {children}
      <span role="tooltip">{content}</span>
    </span>
  );
}
