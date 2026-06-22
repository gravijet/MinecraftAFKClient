package com.hugoafk.net;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Thread-sichere Verwaltung der Online-Spielerliste (aus den Player-Info-Paketen).
 */
public class PlayerList {

    private final Map<UUID, String> names = new ConcurrentHashMap<>();
    private final Map<UUID, Integer> latency = new ConcurrentHashMap<>();

    public void add(UUID id, String name) {
        names.put(id, name);
    }

    public void setLatency(UUID id, int ms) {
        latency.put(id, ms);
    }

    /** Entfernt einen Spieler und gibt seinen letzten bekannten Namen zurueck (oder null). */
    public String remove(UUID id) {
        latency.remove(id);
        return names.remove(id);
    }

    public boolean isKnown(UUID id) {
        return names.containsKey(id);
    }

    public String nameOf(UUID id) {
        return names.get(id);
    }

    public int latencyOf(UUID id) {
        return latency.getOrDefault(id, -1);
    }

    public int size() {
        return names.size();
    }

    public List<String> sortedNames() {
        List<String> list = new ArrayList<>(names.values());
        list.sort(Comparator.comparing(String::toLowerCase));
        return list;
    }

    public void clear() {
        names.clear();
        latency.clear();
    }
}
