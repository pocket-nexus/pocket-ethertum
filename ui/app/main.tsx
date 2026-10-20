// @title Pocket Ethertum
// One screen with a pad: the PS Vita (and a PSP). pocket.json compiles this
// entry wherever no other presentation matches the device's modality.
import { mount } from "@pocketjs/framework";
import SingleScreen from "./presentations/single.tsx";

mount(() => <SingleScreen />);
