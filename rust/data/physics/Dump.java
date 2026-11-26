// Dumps the movement-relevant properties of every block state from Mojang's own code.
//
// Run against an unobfuscated (26.1+) server jar with its bundled libraries on the class path:
//   java -cp "<server-x.jar>:<libraries>" Dump.java > physics-<version>.tsv
// (data/physics/generate.py does exactly that for every version.)
//
// One line per block state, in network id order:
//   id <TAB> block name <TAB> friction <TAB> speed factor <TAB> jump factor <TAB> fluid <TAB> boxes
// fluid: - | water | lava (the fluid state's type, with "f" appended when it is a falling/flowing one)
// boxes: minX,minY,minZ,maxX,maxY,maxZ;... in block units, empty for no collision
import java.util.Locale;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.AABB;

public class Dump {
    static String num(double v) {
        String s = String.format(Locale.ROOT, "%.6f", v);
        s = s.replaceAll("0+$", "");
        return s.endsWith(".") ? s.substring(0, s.length() - 1) : s;
    }

    static <T extends Comparable<T>> String value(BlockState state, net.minecraft.world.level.block.state.properties.Property<T> p) {
        return p.getName(state.getValue(p));
    }

    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        StringBuilder out = new StringBuilder();
        for (int id = 0; id < Block.BLOCK_STATE_REGISTRY.size(); id++) {
            BlockState state = Block.BLOCK_STATE_REGISTRY.byId(id);
            Block block = state.getBlock();
            FluidState fluid = state.getFluidState();
            String fluidName = "-";
            String fluidKey = BuiltInRegistries.FLUID.getKey(fluid.getType()).getPath();
            if (fluidKey.endsWith("water")) fluidName = "water";
            else if (fluidKey.endsWith("lava")) fluidName = "lava";
            if (!fluidName.equals("-")) fluidName += ":" + fluid.getAmount() + (fluid.isSource() ? "" : "f");
            StringBuilder boxes = new StringBuilder();
            for (AABB box : state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).toAabbs()) {
                if (boxes.length() > 0) boxes.append(';');
                boxes.append(num(box.minX)).append(',').append(num(box.minY)).append(',').append(num(box.minZ)).append(',')
                     .append(num(box.maxX)).append(',').append(num(box.maxY)).append(',').append(num(box.maxZ));
            }
            java.util.TreeMap<String, String> props = new java.util.TreeMap<>();
            for (net.minecraft.world.level.block.state.properties.Property<?> p : state.getProperties()) {
                props.put(p.getName(), value(state, p));
            }
            StringBuilder propText = new StringBuilder();
            props.forEach((k, v) -> propText.append(propText.length() > 0 ? "," : "").append(k).append('=').append(v));
            out.append(id).append('\t')
               .append(BuiltInRegistries.BLOCK.getKey(block).getPath()).append('\t')
               .append(propText).append('\t')
               .append(num(block.getFriction())).append('\t')
               .append(num(block.getSpeedFactor())).append('\t')
               .append(num(block.getJumpFactor())).append('\t')
               .append(fluidName).append('\t')
               .append(boxes).append('\n');
        }
        System.out.print(out);
    }
}
