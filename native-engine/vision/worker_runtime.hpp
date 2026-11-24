#pragma once
// Win32 capture, occlusion permission verification, VTP1 codec, and bounded
// multi-region Analyzer runtime for OverlayVision.exe.
// Mirrors visual_tracking/capture.py and visual_tracking/live_worker.py.

#include <windows.h>
#include <dwmapi.h>
#include <wincrypt.h>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <functional>
#include <list>
#include <map>
#include <set>
#include <stdexcept>
#include <string>
#include <tuple>
#include <vector>

#include "../third_party/json.hpp"
#include "image_ops.hpp"

namespace vision {

using json = nlohmann::json;

inline uint64_t perf_counter_ns() {
    LARGE_INTEGER counter{}, freq{};
    QueryPerformanceCounter(&counter);
    QueryPerformanceFrequency(&freq);
    uint64_t ticks = static_cast<uint64_t>(counter.QuadPart);
    uint64_t hz = static_cast<uint64_t>(freq.QuadPart);
    uint64_t secs = ticks / hz;
    uint64_t rem = ticks % hz;
    return secs * 1000000000ULL + (rem * 1000000000ULL) / hz;
}

inline std::string bytes_to_hex(const std::vector<uint8_t>& bytes) {
    static const char kHex[] = "0123456789abcdef";
    std::string out;
    out.reserve(bytes.size() * 2);
    for (uint8_t b : bytes) {
        out.push_back(kHex[b >> 4]);
        out.push_back(kHex[b & 15]);
    }
    return out;
}

inline std::vector<uint8_t> hex_to_bytes(const std::string& hex, size_t max_chars) {
    if (hex.size() > max_chars || (hex.size() % 2) != 0)
        throw std::runtime_error("invalid-template");
    std::vector<uint8_t> out;
    out.reserve(hex.size() / 2);
    auto nib = [](char c) -> int {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return 10 + (c - 'a');
        if (c >= 'A' && c <= 'F') return 10 + (c - 'A');
        return -1;
    };
    for (size_t i = 0; i < hex.size(); i += 2) {
        int hi = nib(hex[i]), lo = nib(hex[i + 1]);
        if (hi < 0 || lo < 0) throw std::runtime_error("invalid-template");
        out.push_back(static_cast<uint8_t>((hi << 4) | lo));
    }
    return out;
}

inline std::string sha256_hex(const std::vector<uint8_t>& bytes) {
    HCRYPTPROV provider = 0;
    HCRYPTHASH hash = 0;
    if (!CryptAcquireContextW(&provider, nullptr, nullptr, PROV_RSA_AES, CRYPT_VERIFYCONTEXT))
        throw std::runtime_error("hash-unavailable");
    try {
        if (!CryptCreateHash(provider, CALG_SHA_256, 0, 0, &hash) ||
            !CryptHashData(hash, bytes.data(), DWORD(bytes.size()), 0))
            throw std::runtime_error("hash-failed");
        uint8_t digest[32]{};
        DWORD len = sizeof(digest);
        if (!CryptGetHashParam(hash, HP_HASHVAL, digest, &len, 0) || len != 32)
            throw std::runtime_error("hash-failed");
        CryptDestroyHash(hash);
        CryptReleaseContext(provider, 0);
        return bytes_to_hex(std::vector<uint8_t>(digest, digest + 32));
    } catch (...) {
        if (hash) CryptDestroyHash(hash);
        CryptReleaseContext(provider, 0);
        throw;
    }
}

inline std::vector<uint8_t> encode_vtp1(const GrayImage& tpl) {
    if (tpl.empty() || tpl.size() > kMaxTemplatePixels)
        throw std::runtime_error("template-pixel-budget-exceeded");
    std::vector<uint8_t> out;
    out.reserve(12 + tpl.data.size());
    out.insert(out.end(), {'V', 'T', 'P', '1'});
    uint32_t dims[2] = {static_cast<uint32_t>(tpl.w), static_cast<uint32_t>(tpl.h)};
    for (uint32_t d : dims) {
        for (int b = 0; b < 4; ++b) out.push_back(static_cast<uint8_t>((d >> (b * 8)) & 0xFF));
    }
    out.insert(out.end(), tpl.data.begin(), tpl.data.end());
    return out;
}

inline GrayImage decode_vtp1_hex(const std::string& hex) {
    std::vector<uint8_t> data = hex_to_bytes(hex, 262168);
    if (data.size() < 13 || data[0] != 'V' || data[1] != 'T' || data[2] != 'P' || data[3] != '1')
        throw std::runtime_error("invalid-template");
    uint32_t w = 0, h = 0;
    for (int b = 0; b < 4; ++b) {
        w |= static_cast<uint32_t>(data[4 + b]) << (b * 8);
        h |= static_cast<uint32_t>(data[8 + b]) << (b * 8);
    }
    if (w == 0 || h == 0 || uint64_t(w) * uint64_t(h) > uint64_t(kMaxTemplatePixels) ||
        data.size() != 12 + size_t(w) * size_t(h))
        throw std::runtime_error("invalid-template");
    GrayImage out;
    out.w = int(w);
    out.h = int(h);
    out.data.assign(data.begin() + 12, data.end());
    return out;
}

inline void dpi_init() {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (!SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2))
        throw std::runtime_error("dpi-init-failed");
}

inline RectI client_rect_screen(HWND hwnd) {
    RECT rect{};
    POINT origin{};
    if (!GetClientRect(hwnd, &rect) || !ClientToScreen(hwnd, &origin))
        throw std::runtime_error("client-geometry-failed");
    return {origin.x, origin.y, rect.right, rect.bottom};
}

// Mirrors visual_tracking/capture.py:grab()
inline GrayImage grab_gray(const RectI& rect, int pixel_limit = kMaxRoiPixels) {
    if (rect.w < 1 || rect.h < 1 || int64_t(rect.w) * int64_t(rect.h) > pixel_limit)
        throw std::runtime_error("capture-pixel-budget-exceeded");
    HDC dc = GetDC(nullptr);
    if (!dc) throw std::runtime_error("capture-dc-failed");
    HDC mem = nullptr;
    HBITMAP bmp = nullptr;
    HGDIOBJ old = nullptr;
    try {
        mem = CreateCompatibleDC(dc);
        if (!mem) throw std::runtime_error("capture-memdc-failed");
        bmp = CreateCompatibleBitmap(dc, rect.w, rect.h);
        if (!bmp) throw std::runtime_error("capture-bitmap-failed");
        old = SelectObject(mem, bmp);
        if (!old || old == HGDI_ERROR) throw std::runtime_error("capture-select-failed");
        if (!BitBlt(mem, 0, 0, rect.w, rect.h, dc, rect.x, rect.y, SRCCOPY | CAPTUREBLT))
            throw std::runtime_error("capture-bitblt-failed");
        SelectObject(mem, old);
        old = nullptr;

        BITMAPINFOHEADER bih{};
        bih.biSize = sizeof(BITMAPINFOHEADER);
        bih.biWidth = rect.w;
        bih.biHeight = -rect.h; // top-down
        bih.biPlanes = 1;
        bih.biBitCount = 32;
        bih.biCompression = BI_RGB;

        std::vector<uint8_t> bgra(size_t(rect.w) * size_t(rect.h) * 4);
        if (GetDIBits(mem, bmp, 0, UINT(rect.h), bgra.data(),
                      reinterpret_cast<BITMAPINFO*>(&bih), DIB_RGB_COLORS) != rect.h)
            throw std::runtime_error("capture-read-failed");

        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(nullptr, dc);
        return bgrx_to_gray(bgra.data(), rect.w, rect.h);
    } catch (...) {
        if (mem && old && old != HGDI_ERROR) SelectObject(mem, old);
        if (bmp) DeleteObject(bmp);
        if (mem) DeleteDC(mem);
        ReleaseDC(nullptr, dc);
        throw;
    }
}

struct OcclusionQuery {
    HWND target = nullptr;
    int rx0 = 0, ry0 = 0, rx1 = 0, ry1 = 0;
    DWORD owner_pid = 0;
    bool positive = false;
    std::set<uintptr_t> allowed_surfaces;
    bool found = false;
    bool blocked = false;
    json diagnostic = json::object();
};

inline BOOL CALLBACK permission_enum_proc(HWND other, LPARAM lparam) {
    auto* q = reinterpret_cast<OcclusionQuery*>(lparam);
    if (other == q->target) {
        q->found = true;
        return FALSE;
    }
    if (!IsWindowVisible(other) || IsIconic(other)) return TRUE;
    DWORD cloak = 0;
    if (DwmGetWindowAttribute(other, DWMWA_CLOAKED, &cloak, sizeof(cloak)) < 0) {
        q->diagnostic = {{"reason", "occluder_cloak_query_failed"},
                         {"window", reinterpret_cast<uintptr_t>(other)}};
        q->blocked = true;
        return FALSE;
    }
    if (cloak) return TRUE;
    RECT box{};
    if (!GetWindowRect(other, &box)) {
        q->diagnostic = {{"reason", "occluder_geometry_query_failed"},
                         {"window", reinterpret_cast<uintptr_t>(other)}};
        q->blocked = true;
        return FALSE;
    }
    if (std::min<long>(q->rx1, box.right) <= std::max<long>(q->rx0, box.left) ||
        std::min<long>(q->ry1, box.bottom) <= std::max<long>(q->ry0, box.top))
        return TRUE;

    DWORD pid = 0;
    GetWindowThreadProcessId(other, &pid);
    DWORD mode = 0;
    if (q->allowed_surfaces.count(reinterpret_cast<uintptr_t>(other)) &&
        pid == q->owner_pid &&
        (q->positive || (GetWindowDisplayAffinity(other, &mode) && mode == WDA_EXCLUDEFROMCAPTURE))) {
        return TRUE;
    }
    q->blocked = true;
    q->diagnostic = {
        {"reason", "roi_occluded"},
        {"window", reinterpret_cast<uintptr_t>(other)},
        {"pid", pid},
        {"rect", {box.left, box.top, box.right, box.bottom}},
    };
    return FALSE;
}

// Mirrors visual_tracking/live_worker.py:permission()
inline bool check_permission(const json& context, bool positive, json& diagnostic_out) {
    diagnostic_out = json::object();
    HWND hwnd = reinterpret_cast<HWND>(context.at("hwnd").get<uintptr_t>());
    const auto& exp = context.at("client");
    if (!exp.is_array() || exp.size() != 4) throw std::runtime_error("invalid-client-rect");
    int ex = exp[0].get<int>(), ey = exp[1].get<int>(), ew = exp[2].get<int>(), eh = exp[3].get<int>();

    if (!IsWindow(hwnd)) { diagnostic_out = {{"reason", "target_destroyed"}}; return false; }
    RectI actual = client_rect_screen(hwnd);
    if (actual.x != ex || actual.y != ey || actual.w != ew || actual.h != eh) {
        diagnostic_out = {{"reason", "client_geometry_changed"},
                          {"expected", {ex, ey, ew, eh}},
                          {"actual", {actual.x, actual.y, actual.w, actual.h}}};
        return false;
    }
    UINT dpi = GetDpiForWindow(hwnd);
    if (dpi != context.at("dpi").get<UINT>()) {
        diagnostic_out = {{"reason", "target_dpi_changed"},
                          {"expected", context.at("dpi")},
                          {"actual", dpi}};
        return false;
    }
    HWND fg = GetAncestor(GetForegroundWindow(), GA_ROOT);
    if (fg != hwnd) {
        diagnostic_out = {{"reason", "target_not_foreground"},
                          {"foreground", reinterpret_cast<uintptr_t>(fg)}};
        return false;
    }
    if (IsIconic(hwnd)) { diagnostic_out = {{"reason", "target_minimized"}}; return false; }
    if (!IsWindowVisible(hwnd)) { diagnostic_out = {{"reason", "target_hidden"}}; return false; }
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    if (pid != context.at("pid").get<DWORD>()) {
        diagnostic_out = {{"reason", "target_process_changed"}};
        return false;
    }

    const auto& r = context.at("roi");
    if (!r.is_array() || r.size() != 4 || !r[0].is_number_integer() || !r[1].is_number_integer() ||
        !r[2].is_number_integer() || !r[3].is_number_integer())
        throw std::runtime_error("invalid-authorized-roi");
    int rx = r[0].get<int>(), ry = r[1].get<int>(), rw = r[2].get<int>(), rh = r[3].get<int>();
    if (rw < 1 || rh < 1 || int64_t(rw) * int64_t(rh) > kMaxRoiPixels ||
        rx < ex || ry < ey || rx + rw > ex + ew || ry + rh > ey + eh)
        throw std::runtime_error("invalid-authorized-roi");

    OcclusionQuery query;
    query.target = hwnd;
    query.rx0 = rx; query.ry0 = ry; query.rx1 = rx + rw; query.ry1 = ry + rh;
    query.owner_pid = context.at("owner_pid").get<DWORD>();
    query.positive = positive;
    for (const auto& s : context.at("surfaces"))
        query.allowed_surfaces.insert(s.get<uintptr_t>());

    EnumWindows(permission_enum_proc, reinterpret_cast<LPARAM>(&query));
    if (!query.found && !query.blocked)
        diagnostic_out = {{"reason", "target_not_enumerated"}};
    else
        diagnostic_out = query.diagnostic;
    return query.found && !query.blocked;
}

// Mirrors visual_tracking/live_worker.py:Analyzer with injectable capture/permission
// hooks so --self-test can exercise watch, LRU cache, and shared-frame reuse offline.
class Analyzer {
public:
    using PermissionFn = std::function<bool(const json&, bool, json&)>;
    using GrabFn = std::function<GrayImage(const RectI&, int)>;
    using ClockFn = std::function<uint64_t()>;

    PermissionFn permission_fn;
    GrabFn grab_fn;
    ClockFn clock_fn;

    Analyzer()
        : permission_fn(check_permission),
          grab_fn([](const RectI& r, int lim) { return grab_gray(r, lim); }),
          clock_fn(perf_counter_ns) {}

    struct WatchKey {
        std::string region_id;
        std::string calibration_id;
        uint64_t generation = 0;
        bool operator==(const WatchKey& o) const {
            return region_id == o.region_id && calibration_id == o.calibration_id && generation == o.generation;
        }
    };
    struct WatchEntry {
        WatchKey key;
        GrayImage image;
        json context;
    };
    struct ShareIdentity {
        uintptr_t hwnd = 0;
        uint32_t pid = 0;
        int cx = 0, cy = 0, cw = 0, ch = 0;
        uint32_t dpi = 0;
        uint64_t generation = 0;
        uint32_t owner_pid = 0;
        std::vector<uintptr_t> surfaces;
        std::vector<std::string> allowed;
        std::vector<std::pair<std::string, std::string>> calibrations;
        bool valid = false;
        bool operator==(const ShareIdentity& o) const {
            return valid && o.valid && hwnd == o.hwnd && pid == o.pid &&
                   cx == o.cx && cy == o.cy && cw == o.cw && ch == o.ch &&
                   dpi == o.dpi && generation == o.generation && owner_pid == o.owner_pid &&
                   surfaces == o.surfaces && allowed == o.allowed && calibrations == o.calibrations;
        }
    };
    struct AnalysisKey {
        int rx = 0, ry = 0, rw = 0, rh = 0;
        std::string template_sha256;
        double ratio = 1.0;
        std::string tracking_kind;
        bool operator<(const AnalysisKey& o) const {
            return std::tie(rx, ry, rw, rh, template_sha256, ratio, tracking_kind) <
                   std::tie(o.rx, o.ry, o.rw, o.rh, o.template_sha256, o.ratio, o.tracking_kind);
        }
    };
    struct SharedFrame {
        GrayImage image;
        ShareIdentity identity;
        int rx = 0, ry = 0, rw = 0, rh = 0;
        uint64_t sample = 0;
        uint64_t time_ns = 0;
        std::set<std::string> used;
        std::map<AnalysisKey, LocateResult> analyses;
    };

    uint64_t sample = 0;
    bool has_baseline = false;
    GrayImage baseline;
    json baseline_context;
    std::list<WatchEntry> watches;
    bool has_shared = false;
    SharedFrame shared;

    void expire_shared() {
        if (!has_shared) return;
        uint64_t now = clock_fn();
        if (now < shared.time_ns || (now - shared.time_ns) >= 100000000ULL)
            has_shared = false;
    }

    int shared_pixels() const { return has_shared ? shared.image.size() : 0; }
    int watch_pixels() const {
        int total = 0;
        for (const auto& w : watches) total += w.image.size();
        return total;
    }
    int retained_pixels() const {
        return watch_pixels() + (has_baseline ? baseline.size() : 0) + shared_pixels();
    }

    static ShareIdentity make_share_identity(const json& context) {
        if (!context.contains("cache_regions") || !context["cache_regions"].is_array()) return {};
        if (!context.contains("cache_calibrations") || !context["cache_calibrations"].is_object()) return {};
        const auto& allowed_arr = context["cache_regions"];
        if (allowed_arr.size() <= 1 || allowed_arr.size() > size_t(kMaxActiveRegions)) return {};
        const auto& cals = context["cache_calibrations"];
        if (cals.size() != allowed_arr.size()) return {};
        std::set<std::string> allowed_set;
        for (const auto& v : allowed_arr) {
            if (!v.is_string()) return {};
            auto s = v.get<std::string>();
            if (s.size() > 128) return {};
            allowed_set.insert(s);
        }
        std::vector<std::pair<std::string, std::string>> cal_pairs;
        for (auto it = cals.begin(); it != cals.end(); ++it) {
            if (!allowed_set.count(it.key()) || !it.value().is_string()) return {};
            auto val = it.value().get<std::string>();
            if (val.size() > 128) return {};
            cal_pairs.emplace_back(it.key(), val);
        }
        if (cal_pairs.size() != allowed_set.size()) return {};
        std::string reg = context.value("region_id", std::string());
        auto it = cals.find(reg);
        if (it == cals.end() || it->get<std::string>() != context.value("calibration_id", std::string()))
            return {};

        ShareIdentity id;
        id.valid = true;
        id.hwnd = context.at("hwnd").get<uintptr_t>();
        id.pid = context.at("pid").get<uint32_t>();
        const auto& cl = context.at("client");
        id.cx = cl[0].get<int>(); id.cy = cl[1].get<int>();
        id.cw = cl[2].get<int>(); id.ch = cl[3].get<int>();
        id.dpi = context.at("dpi").get<uint32_t>();
        id.generation = context.at("generation").get<uint64_t>();
        id.owner_pid = context.at("owner_pid").get<uint32_t>();
        for (const auto& s : context.at("surfaces")) id.surfaces.push_back(s.get<uintptr_t>());
        id.allowed.assign(allowed_set.begin(), allowed_set.end());
        std::sort(cal_pairs.begin(), cal_pairs.end());
        id.calibrations = std::move(cal_pairs);
        return id;
    }

    static WatchKey make_watch_key(const json& context) {
        return {context.value("region_id", std::string("default")),
                context.value("calibration_id", std::string("")),
                context.at("generation").get<uint64_t>()};
    }

    bool trim_cache(const json& context, int reserve = 0) {
        std::vector<std::string> allowed;
        if (context.contains("cache_regions")) {
            if (!context["cache_regions"].is_array() || context["cache_regions"].size() > size_t(kMaxActiveRegions))
                throw std::runtime_error("invalid-cache-budget");
            for (const auto& v : context["cache_regions"]) {
                if (!v.is_string() || v.get<std::string>().size() > 128)
                    throw std::runtime_error("invalid-cache-budget");
                allowed.push_back(v.get<std::string>());
            }
        } else {
            allowed.push_back(context.value("region_id", std::string("default")));
        }
        int budget = kMaxRetainedPixels;
        if (context.contains("cache_budget_pixels")) {
            const auto& b = context["cache_budget_pixels"];
            if (!b.is_number_integer() || b.is_boolean()) throw std::runtime_error("invalid-cache-budget");
            budget = b.get<int>();
            if (budget < 0 || budget > kMaxRetainedPixels) throw std::runtime_error("invalid-cache-budget");
        }
        expire_shared();
        if (has_shared && (!(shared.identity == make_share_identity(context)) ||
                           watch_pixels() + reserve + shared_pixels() > budget)) {
            has_shared = false;
        }
        std::string cur_reg = context.value("region_id", std::string("default"));
        std::string cur_cal = context.value("calibration_id", std::string(""));
        uint64_t cur_gen = context.at("generation").get<uint64_t>();
        for (auto it = watches.begin(); it != watches.end();) {
            bool in_allowed = std::find(allowed.begin(), allowed.end(), it->key.region_id) != allowed.end();
            bool mismatch = !in_allowed || it->key.generation != cur_gen ||
                            (it->key.region_id == cur_reg && it->key.calibration_id != cur_cal);
            if (mismatch) it = watches.erase(it);
            else ++it;
        }
        while (!watches.empty() && watch_pixels() + reserve + shared_pixels() > budget)
            watches.pop_front();
        return reserve + shared_pixels() <= budget;
    }

    void remember_frame(const json& context, const GrayImage& image, uint64_t samp, uint64_t sampled_ns) {
        ShareIdentity id = make_share_identity(context);
        int budget = context.value("cache_budget_pixels", kMaxRetainedPixels);
        if (id.valid && retained_pixels() + image.size() <= budget) {
            has_shared = true;
            shared = SharedFrame{};
            shared.image = image;
            shared.identity = std::move(id);
            const auto& r = context.at("roi");
            shared.rx = r[0].get<int>(); shared.ry = r[1].get<int>();
            shared.rw = r[2].get<int>(); shared.rh = r[3].get<int>();
            shared.sample = samp;
            shared.time_ns = sampled_ns;
            shared.used.insert(context.value("region_id", std::string()));
        }
    }

    void put_watch(const WatchKey& key, const GrayImage& image, const json& context) {
        for (auto it = watches.begin(); it != watches.end();) {
            if (it->key.region_id == key.region_id) it = watches.erase(it);
            else ++it;
        }
        if (trim_cache(context, image.size()))
            watches.push_back({key, image, context});
    }

    json execute(const json& task) {
        try {
            return execute_inner(task);
        } catch (...) {
            has_shared = false;
            throw;
        }
    }

private:
    static bool same_proof_context(const json& a, const json& b) {
        for (const char* k : {"hwnd", "pid", "client", "dpi", "roi", "generation"}) {
            if (a.at(k) != b.at(k)) return false;
        }
        return true;
    }

    json execute_inner(const json& task) {
        const auto& context = task.at("context");
        std::string op = task.at("operation").get<std::string>();
        std::string kind = task.value("tracking_kind", std::string("boundary"));
        if (kind != "boundary" && kind != "features")
            throw std::runtime_error("invalid-tracking-kind");
        if (op != "calibrate" && op != "baseline" && op != "positive" &&
            op != "excluded" && op != "locate" && op != "watch")
            throw std::runtime_error("unsupported-operation");

        bool ordinary = (op == "locate" || op == "watch");
        uint64_t minimum_sample = 0;
        if (task.contains("minimum_sample_t_ns")) {
            const auto& ms = task["minimum_sample_t_ns"];
            if (!ms.is_number_integer() || ms.is_boolean() || ms.get<int64_t>() < 0)
                throw std::runtime_error("invalid-minimum-sample-time");
            minimum_sample = ms.get<uint64_t>();
        }
        if (!ordinary) has_shared = false;
        if (op == "calibrate" || op == "baseline") {
            watches.clear();
            has_baseline = false;
            baseline = GrayImage{};
            baseline_context = json();
        }
        trim_cache(context, has_baseline ? baseline.size() : 0);

        bool positive = (op == "positive");
        json diag;
        if (!permission_fn(context, positive, diag)) {
            has_shared = false;
            return {{"status", "paused"}, {"reason", "capture_permission_revoked"}, {"permission_detail", diag}};
        }

        const auto& roi_arr = context.at("roi");
        int rx = roi_arr[0].get<int>(), ry = roi_arr[1].get<int>();
        int rw = roi_arr[2].get<int>(), rh = roi_arr[3].get<int>();
        std::string region_id = context.value("region_id", std::string());

        bool reused = false;
        if (ordinary && has_shared) {
            expire_shared();
            if (has_shared) {
                reused = !region_id.empty() && !shared.used.count(region_id) &&
                         shared.time_ns >= minimum_sample &&
                         shared.rx <= rx && shared.ry <= ry &&
                         rx + rw <= shared.rx + shared.rw &&
                         ry + rh <= shared.ry + shared.rh;
            }
        }

        uint64_t sampled_ns = 0, samp_id = 0;
        GrayImage image;
        if (reused) {
            sampled_ns = shared.time_ns;
            samp_id = shared.sample;
            image = shared.image.crop(rx - shared.rx, ry - shared.ry, rw, rh);
        } else {
            has_shared = false;
            sampled_ns = clock_fn();
            image = grab_fn({rx, ry, rw, rh}, kMaxRoiPixels);
        }

        if (!permission_fn(context, positive, diag)) {
            has_shared = false;
            return {{"status", "paused"}, {"reason", "capture_permission_revoked"}, {"permission_detail", diag}};
        }
        if (reused) shared.used.insert(region_id);
        else samp_id = ++sample;

        json result = {
            {"sample_id", samp_id},
            {"sample_t_ns", sampled_ns},
            {"context", context},
            {"capture_reused", reused},
            {"capture_roi", {rx, ry, rw, rh}},
        };

        if (op == "watch") {
            WatchKey key = make_watch_key(context);
            bool same = false;
            for (const auto& entry : watches) {
                if (entry.key == key && same_proof_context(context, entry.context) && entry.image.equals(image)) {
                    same = true;
                    break;
                }
            }
            result["status"] = same ? "unchanged" : "changed";
            if (!reused) remember_frame(context, image, samp_id, sampled_ns);
            return result;
        }

        if (op == "positive" || op == "excluded") {
            if (!has_baseline || !same_proof_context(context, baseline_context) ||
                image.w != baseline.w || image.h != baseline.h)
                throw std::runtime_error("capture-proof-context-changed");
            double sum = 0.0;
            for (size_t i = 0; i < image.data.size(); ++i)
                sum += std::abs(double(image.data[i]) - double(baseline.data[i]));
            result["status"] = "proof";
            result["difference"] = sum / double(image.data.size());
            return result;
        }

        GrayImage tpl;
        double ratio = 1.0;
        bool has_analysis_key = false;
        AnalysisKey akey;
        if (op == "calibrate" || op == "baseline") {
            if (!trim_cache(context, image.size()))
                throw std::runtime_error("retained-pixel-budget-exceeded");
            baseline = image;
            baseline_context = context;
            has_baseline = true;
            if (op == "baseline") {
                result["status"] = "baseline";
                return result;
            }
            const auto& sel = task.at("selection");
            if (!sel.is_array() || sel.size() != 4 || !sel[0].is_number_integer() ||
                !sel[1].is_number_integer() || !sel[2].is_number_integer() || !sel[3].is_number_integer())
                throw std::runtime_error("invalid-selection-rect");
            RectI srect{sel[0].get<int>(), sel[1].get<int>(), sel[2].get<int>(), sel[3].get<int>()};
            tpl = learn_template(image, srect, kind);
            std::vector<uint8_t> vtp = encode_vtp1(tpl);
            result["template_hex"] = bytes_to_hex(vtp);
            result["template_sha256"] = sha256_hex(vtp);
        } else { // locate
            std::string tpl_hex = task.at("template_hex").get<std::string>();
            tpl = decode_vtp1_hex(tpl_hex);
            double cal_dpi = task.at("calibration_dpi").get<double>();
            double cur_dpi = context.at("dpi").get<double>();
            if (!std::isfinite(cal_dpi) || cal_dpi < 48.0 || cal_dpi > 768.0)
                throw std::runtime_error("invalid-dpi");
            ratio = cur_dpi / cal_dpi;
            if (ratio != 1.0) {
                int nw = std::max(1, static_cast<int>(std::llround(double(tpl.w) * ratio)));
                int nh = std::max(1, static_cast<int>(std::llround(double(tpl.h) * ratio)));
                tpl = resize_linear(tpl, nw, nh);
            }
            has_baseline = false;
            baseline = GrayImage{};
            baseline_context = json();
            std::vector<uint8_t> ascii_bytes(tpl_hex.begin(), tpl_hex.end());
            akey = {rx, ry, rw, rh, sha256_hex(ascii_bytes), ratio, kind};
            has_analysis_key = true;
        }

        bool analysis_reused = false;
        LocateResult loc;
        if (has_analysis_key && has_shared) {
            auto it = shared.analyses.find(akey);
            if (it != shared.analyses.end()) {
                loc = it->second;
                analysis_reused = true;
            }
        }
        if (!analysis_reused) loc = locate_template(image, tpl, kind);

        result["analysis_reused"] = analysis_reused;
        result["status"] = loc.found ? "complete" : "not_found";
        result["reason"] = loc.reason;
        result["score"] = loc.score;
        result["unique"] = loc.found;
        result["boundary_checked"] = loc.found && kind == "boundary";
        result["position_checked"] = loc.found && kind == "features";
        if (loc.found) {
            result["boundary_rect"] = {loc.rect.x + rx, loc.rect.y + ry, loc.rect.w, loc.rect.h};
            if (op == "locate") put_watch(make_watch_key(context), image, context);
        } else {
            result["boundary_rect"] = nullptr;
        }
        if (ordinary && !reused) remember_frame(context, image, samp_id, sampled_ns);
        if (has_analysis_key && has_shared) shared.analyses[akey] = loc;
        return result;
    }
};

} // namespace vision
