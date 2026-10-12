export type SignalChainItem = {
  slot: number;
  name: string;
  modelName: string;
  enabled: boolean;
  rack: boolean;
};

type Props = {
  items: SignalChainItem[];
  selected: number;
  onSelect: (slot: number) => void;
};

export default function SignalChain({ items, selected, onSelect }: Props) {
  return <section className="chain-section">
    <div className="section-title"><div><small>SIGNAL PATH · 09 BLOCKS</small><h2>Signal chain</h2></div><span>SELECT A BLOCK TO EDIT</span></div>
    <div className="chain-scroll"><div className="chain">
      {items.map((item) => <button
        key={item.slot}
        type="button"
        aria-pressed={selected === item.slot}
        aria-label={`${item.name}: ${item.modelName || "no model"}, ${item.enabled ? "active" : "bypassed"}`}
        title={`${item.name} · ${item.modelName || "No model"}`}
        onClick={() => onSelect(item.slot)}
        className={`chain-module ${item.rack ? `rack-module ${item.slot === 3 ? "cab-rack" : "amp-rack"}` : "stomp-pedal"} ${selected === item.slot ? "active" : ""}`}>
        {item.rack ? <>
          <span className="rack-topline"><span className="rack-label">{item.slot === 2 ? "AMPLIFIER" : "CABINET"}</span><i className={`rack-led ${item.enabled ? "on" : ""}`} aria-hidden="true" /></span>
          <b>{item.modelName || "—"}</b>
          <span className="pedal-state">{item.enabled ? "ACTIVE" : "BYPASS"}</span>
        </> : <>
          <span className="pedal-topline"><i className={`module-led ${item.enabled ? "on" : ""}`} /><small>STOMPBOX</small></span>
          <span className="pedal-kind">{item.name}</span>
          <b>{item.modelName || "—"}</b>
          <span className="pedal-state">{item.enabled ? "ACTIVE" : "BYPASS"}</span>
        </>}
      </button>)}
    </div></div>
  </section>;
}
