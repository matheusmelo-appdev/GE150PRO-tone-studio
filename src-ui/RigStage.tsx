type ModelOption = { name: string; disabled: boolean };

type Props = {
  block: string;
  blockNumber: number;
  modelName: string;
  modelIndex?: number;
  modelOptions: ModelOption[];
  image: string;
  hasPreset: boolean;
  onModelChange: (index: number) => void;
};

export default function RigStage({ block, blockNumber, modelName, modelIndex, modelOptions, image, hasPreset, onModelChange }: Props) {
  const gearType = block === "AMP" ? "AMPLIFIER HEAD" : block === "CAB" ? "SPEAKER CABINET" : "EFFECT PEDAL";

  return <section className="rig-stage" aria-label="Selected gear">
    <div className="rig-stage-art"><img src={image} alt="" /></div>
    <div className="rig-stage-info">
      <div className="rig-stage-kicker"><span>{gearType}</span><span>GE150 · BLOCK {String(blockNumber).padStart(2, "0")}</span></div>
      <h2>{hasPreset ? modelName || "Select a model" : "Your rig starts here"}</h2>
      <p>{hasPreset ? "Selected GE150 model · choose another model below" : "Load a pedal preset or create a tone with the assistant"}</p>
      {modelIndex !== undefined && <label className="rig-model-picker">
        <span>MODEL</span>
        <select aria-label={`${block} model`} value={modelIndex} onChange={(event) => onModelChange(Number(event.target.value))}>
          {modelOptions.map((option, index) => <option key={`${index}-${option.name}`} value={index} disabled={option.disabled}>{option.name}</option>)}
        </select>
      </label>}
      <div className={`rig-stage-ready ${hasPreset ? "loaded" : ""}`}><i />{hasPreset ? "PRESET LOADED" : "WAITING FOR PRESET"}</div>
    </div>
  </section>;
}
