// Reference values for the 26.4 login key exchange, computed with Mojang's own classes.
//
// java -cp "server-26.4-pre-1.jar;guava.jar;failureaccess.jar;commons-codec.jar" Reference.java
// (the jars are inside the official server jar under META-INF/versions and META-INF/libraries)
//
// Fixed inputs: client secret 32 x 0x11, server secret 32 x 0x22, salt 0x0123456789ABCDEF,
// server id "server", client name "Bot", protocol 0x4000015D, authentication requested.
import java.math.BigInteger;
import java.security.KeyFactory;
import java.security.KeyPair;
import java.security.PrivateKey;
import java.security.PublicKey;
import java.security.spec.NamedParameterSpec;
import java.security.spec.XECPrivateKeySpec;
import java.security.spec.XECPublicKeySpec;
import java.util.Arrays;
import java.util.HexFormat;
import javax.crypto.KeyAgreement;
import net.minecraft.util.crypto.PublicPeerInfo;
import net.minecraft.util.crypto.SessionCommonInfo;
import net.minecraft.util.crypto.SessionKeyExchange;
import net.minecraft.util.crypto.SessionKeys;
import net.minecraft.util.crypto.StreamCryptoUtils;

public class Reference {
    static KeyPair pair(byte fill) throws Exception {
        byte[] scalar = new byte[32];
        Arrays.fill(scalar, fill);
        KeyFactory factory = KeyFactory.getInstance("XDH");
        PrivateKey priv = factory.generatePrivate(new XECPrivateKeySpec(NamedParameterSpec.X25519, scalar));
        // public = X25519(scalar, 9): agree with the base point
        KeyAgreement agreement = KeyAgreement.getInstance("XDH");
        agreement.init(priv);
        agreement.doPhase(factory.generatePublic(new XECPublicKeySpec(NamedParameterSpec.X25519, BigInteger.valueOf(9))), true);
        byte[] u = agreement.generateSecret();
        byte[] bigEndian = new byte[32];
        for (int i = 0; i < 32; i++) bigEndian[i] = u[31 - i];
        PublicKey pub = factory.generatePublic(new XECPublicKeySpec(NamedParameterSpec.X25519, new BigInteger(1, bigEndian)));
        return new KeyPair(pub, priv);
    }

    public static void main(String[] args) throws Exception {
        HexFormat hex = HexFormat.of();
        KeyPair client = pair((byte) 0x11);
        KeyPair server = pair((byte) 0x22);
        SessionCommonInfo info = new SessionCommonInfo(0x4000015D, 0x0123456789ABCDEFL, true);
        SessionKeyExchange exchange = new SessionKeyExchange(info, SessionKeyExchange.Side.B, "Bot", client);
        SessionKeys keys = exchange.createSessionKeys(new PublicPeerInfo(server.getPublic(), "server"));
        byte[] signature = keys.computeIntegrityDigest();
        StreamCryptoUtils.IVPair ivs = new StreamCryptoUtils.IVGenerator(signature).generatePair();
        System.out.println("signature=" + hex.formatHex(signature));
        System.out.println("auth=" + keys.computeKeysAuthenticationDigest());
        System.out.println("inbound=" + hex.formatHex(keys.inboundStreamKey(SessionKeyExchange.Side.B).getEncoded()));
        System.out.println("outbound=" + hex.formatHex(keys.outboundStreamKey(SessionKeyExchange.Side.B).getEncoded()));
        System.out.println("inbound_iv=" + hex.formatHex(ivs.serverToClient().getIV()));
        System.out.println("outbound_iv=" + hex.formatHex(ivs.clientToServer().getIV()));
    }
}
