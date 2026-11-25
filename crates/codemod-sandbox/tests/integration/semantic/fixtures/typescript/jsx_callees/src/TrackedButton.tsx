export function TrackedButton({ eventLabel, onClick, children }) {
  return (
    <button data-event={eventLabel} onClick={onClick}>
      {children}
    </button>
  );
}
