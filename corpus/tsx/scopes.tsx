import { useState } from "react";

type Props = { label: string };

export function Counter({ label }: Props) {
  const [count, setCount] = useState(0);
  return (
    <div>
      <Button onClick={() => setCount(count + 1)} />
      {label}
    </div>
  );
}

function Button(props: { onClick: () => void }) {
  return <button onClick={props.onClick}>+</button>;
}
