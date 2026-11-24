// Native C++17 OverlayVision.exe entry point.
// Replaces the PyInstaller/Python/OpenCV visual worker with zero Python runtime
// dependencies while preserving the exact visual-worker-1 named-pipe protocol.

#include "worker_runtime.hpp"

#include <shellapi.h>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <thread>

namespace fs = std::filesystem;

namespace {

constexpr const char* kProtocol = "visual-worker-1";
constexpr size_t kMessageLimit = 1048576;

inline void require_cond(bool ok, const char* msg) {
    if (!ok) throw std::runtime_error(msg);
}

class PipeClient {
public:
    HANDLE handle = INVALID_HANDLE_VALUE;

    PipeClient(const std::wstring& name, DWORD timeout_ms = 2500) {
        uint64_t deadline = GetTickCount64() + timeout_ms;
        for (;;) {
            HANDLE h = CreateFileW(name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr,
                                   OPEN_EXISTING, FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, nullptr);
            if (h != INVALID_HANDLE_VALUE) {
                handle = h;
                break;
            }
            DWORD err = GetLastError();
            if ((err != ERROR_FILE_NOT_FOUND && err != ERROR_PIPE_BUSY) || GetTickCount64() >= deadline)
                throw std::runtime_error("pipe-connect-failed");
            WaitNamedPipeW(name.c_str(), 50);
            Sleep(10);
        }
        DWORD mode = PIPE_READMODE_MESSAGE;
        if (!SetNamedPipeHandleState(handle, &mode, nullptr, nullptr)) {
            CloseHandle(handle);
            handle = INVALID_HANDLE_VALUE;
            throw std::runtime_error("pipe-mode-failed");
        }
    }

    ~PipeClient() {
        if (handle != INVALID_HANDLE_VALUE) CloseHandle(handle);
    }

    PipeClient(const PipeClient&) = delete;
    PipeClient& operator=(const PipeClient&) = delete;

    DWORD server_pid() const {
        ULONG pid = 0;
        if (!GetNamedPipeServerProcessId(handle, &pid)) return 0;
        return DWORD(pid);
    }

    vision::json exchange(const vision::json& req) {
        std::string wire = req.dump(-1, ' ', false, nlohmann::json::error_handler_t::strict);
        if (wire.size() > kMessageLimit) throw std::runtime_error("request-too-large");
        write_all(wire);
        std::string resp_wire = read_message();
        auto parsed = vision::json::parse(resp_wire);
        if (!parsed.is_object()) throw std::runtime_error("response-not-object");
        return parsed;
    }

private:
    void write_all(const std::string& data) {
        HANDLE ev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (!ev) throw std::runtime_error("pipe-event-failed");
        OVERLAPPED ov{};
        ov.hEvent = ev;
        DWORD written = 0;
        BOOL ok = WriteFile(handle, data.data(), DWORD(data.size()), &written, &ov);
        if (!ok) {
            DWORD err = GetLastError();
            if (err != ERROR_IO_PENDING) {
                CloseHandle(ev);
                throw std::runtime_error("pipe-write-failed");
            }
            DWORD w = WaitForSingleObject(ev, 5000);
            if (w != WAIT_OBJECT_0 || !GetOverlappedResult(handle, &ov, &written, FALSE)) {
                CancelIoEx(handle, &ov);
                GetOverlappedResult(handle, &ov, &written, TRUE);
                CloseHandle(ev);
                throw std::runtime_error("pipe-write-timeout");
            }
        }
        CloseHandle(ev);
        if (written != data.size()) throw std::runtime_error("pipe-write-incomplete");
    }

    std::string read_message() {
        std::string chunks;
        std::vector<char> buf(65536);
        for (;;) {
            HANDLE ev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
            if (!ev) throw std::runtime_error("pipe-event-failed");
            OVERLAPPED ov{};
            ov.hEvent = ev;
            DWORD count = 0;
            BOOL ok = ReadFile(handle, buf.data(), DWORD(buf.size()), &count, &ov);
            DWORD err = ok ? 0 : GetLastError();
            if (!ok && err == ERROR_IO_PENDING) {
                DWORD w = WaitForSingleObject(ev, 5000);
                if (w != WAIT_OBJECT_0) {
                    CancelIoEx(handle, &ov);
                    GetOverlappedResult(handle, &ov, &count, TRUE);
                    CloseHandle(ev);
                    throw std::runtime_error("pipe-read-timeout");
                }
                ok = GetOverlappedResult(handle, &ov, &count, FALSE);
                err = ok ? 0 : GetLastError();
            }
            CloseHandle(ev);
            if (err != 0 && err != ERROR_MORE_DATA) throw std::runtime_error("pipe-read-failed");
            chunks.append(buf.data(), count);
            if (chunks.size() > kMessageLimit) throw std::runtime_error("response-too-large");
            if (err == 0) break;
        }
        return chunks;
    }
};

vision::GrayImage make_synth_scene(bool duplicate = false, int dx = 0, int dy = 0, bool dark = false) {
    vision::GrayImage img;
    img.w = 660;
    img.h = 270;
    uint8_t outside = dark ? 24 : 229;
    uint8_t inside = dark ? 38 : 250;
    uint8_t line = dark ? 130 : 117;
    uint8_t ink = dark ? 235 : 37;
    img.data.assign(size_t(img.w) * size_t(img.h), outside);
    auto fill_rect = [&](int x, int y, int w, int h, uint8_t c) {
        for (int yy = y; yy < y + h; ++yy)
            for (int xx = x; xx < x + w; ++xx) img.at(xx, yy) = c;
    };
    auto stroke_rect = [&](int x, int y, int w, int h, uint8_t c) {
        for (int xx = x; xx < x + w; ++xx) { img.at(xx, y) = c; img.at(xx, y + h - 1) = c; }
        for (int yy = y; yy < y + h; ++yy) { img.at(x, yy) = c; img.at(x + w - 1, yy) = c; }
    };
    fill_rect(60 + dx, 60 + dy, 520, 150, inside);
    stroke_rect(60 + dx, 60 + dy, 520, 150, line);
    auto stamp_pattern = [&](int ox, int oy) {
        for (int k = 0; k < 10; ++k) {
            fill_rect(ox + k * 14, oy, 7, 14, ink);
            if (k % 2 == 0) fill_rect(ox + k * 14 + 2, oy + 4, 3, 6, inside);
        }
    };
    stamp_pattern(85 + dx, 95 + dy);
    if (duplicate) stamp_pattern(85 + dx, 155 + dy);
    return img;
}

int run_self_test(const fs::path& output_dir) {
    fs::create_directories(output_dir);

    // 1. Direct vs 2D FFT normalized cross-correlation parity.
    {
        auto frame = make_synth_scene(false, 0, 0, false);
        auto tpl = frame.crop(85, 95, 138, 16);
        int ow = 0, oh = 0;
        auto fft_scores = vision::match_template_ccoeff_normed(frame, tpl, ow, oh);
        double best = -2.0;
        int bx = -1, by = -1;
        for (int y = 0; y < oh; ++y)
            for (int x = 0; x < ow; ++x) {
                double v = fft_scores[size_t(y) * size_t(ow) + size_t(x)];
                if (v > best) { best = v; bx = x; by = y; }
            }
        require_cond(bx == 85 && by == 95 && best > 0.999, "fft-ncc-mismatch");
    }

    // 2. Boundary and feature learn + locate parity.
    {
        for (bool dark : {false, true}) {
            auto frame = make_synth_scene(false, 0, 0, dark);
            auto tpl = vision::learn_template(frame, {60, 60, 520, 150}, "boundary");
            auto res = vision::locate_template(frame, tpl, "boundary");
            require_cond(res.found && res.reason == "snapshot-supported" &&
                             std::abs(res.rect.x - 58) <= 2 && std::abs(res.rect.y - 58) <= 2 &&
                             std::abs(res.rect.w - 524) <= 4 && std::abs(res.rect.h - 154) <= 4,
                         "boundary-locate-failed");
        }
        auto dup_frame = make_synth_scene(true, 0, 0, false);
        auto feat_tpl = vision::learn_template(make_synth_scene(), {82, 92, 145, 22}, "features");
        auto dup_res = vision::locate_template(dup_frame, feat_tpl, "features");
        require_cond(!dup_res.found && dup_res.reason == "ambiguous-features", "duplicate-not-rejected");
        auto moved = make_synth_scene(false, 18, 12, false);
        auto moved_res = vision::locate_template(moved, feat_tpl, "features");
        require_cond(moved_res.found && moved_res.rect.x == 100 && moved_res.rect.y == 104,
                     "feature-moved-mismatch");
    }

    // 3. VTP1 roundtrip & SHA-256.
    {
        auto tpl = vision::learn_template(make_synth_scene(), {82, 92, 145, 22}, "features");
        auto bytes = vision::encode_vtp1(tpl);
        auto hex = vision::bytes_to_hex(bytes);
        auto decoded = vision::decode_vtp1_hex(hex);
        require_cond(decoded.equals(tpl) && vision::sha256_hex(bytes).size() == 64, "vtp1-roundtrip-failed");
    }

    // 4. Analyzer watch, shared capture reuse, and analysis reuse.
    {
        vision::Analyzer analyzer;
        uint64_t fake_now = 1000000000ULL;
        int grabs = 0;
        auto scene = make_synth_scene();
        analyzer.clock_fn = [&]() { return fake_now; };
        analyzer.permission_fn = [](const vision::json&, bool, vision::json& d) { d = vision::json::object(); return true; };
        analyzer.grab_fn = [&](const vision::RectI& r, int) {
            ++grabs;
            return scene.crop(r.x, r.y, r.w, r.h);
        };

        auto ctx_base = [&](const std::string& rid, const std::string& cal, std::vector<int> roi) {
            return vision::json{
                {"hwnd", 123}, {"pid", 456}, {"client", {0, 0, 660, 270}}, {"dpi", 96},
                {"roi", roi}, {"generation", 1}, {"owner_pid", 789},
                {"surfaces", vision::json::array()}, {"region_id", rid}, {"calibration_id", cal},
                {"cache_regions", {"r1", "r2"}},
                {"cache_calibrations", {{"r1", "c1"}, {"r2", "c2"}}},
                {"cache_budget_pixels", 2097152}
            };
        };

        auto cal_res = analyzer.execute({{"operation", "calibrate"}, {"tracking_kind", "features"},
                                         {"selection", {82, 92, 145, 22}},
                                         {"context", ctx_base("r1", "c1", {0, 0, 660, 270})}});
        require_cond(cal_res["status"] == "complete", "self-test-calibrate-failed");

        auto loc1 = analyzer.execute({{"operation", "locate"}, {"tracking_kind", "features"},
                                      {"template_hex", cal_res["template_hex"]}, {"calibration_dpi", 96},
                                      {"context", ctx_base("r1", "c1", {40, 40, 560, 200})}});
        require_cond(loc1["status"] == "complete" && !loc1["capture_reused"].get<bool>(), "loc1-failed");

        fake_now += 20000000ULL; // +20ms (< 100ms shared window)
        auto loc2 = analyzer.execute({{"operation", "locate"}, {"tracking_kind", "features"},
                                      {"template_hex", cal_res["template_hex"]}, {"calibration_dpi", 96},
                                      {"context", ctx_base("r2", "c2", {60, 60, 200, 100})}});
        require_cond(loc2["status"] == "complete" && loc2["capture_reused"].get<bool>() && grabs == 2,
                     "shared-capture-reuse-failed");

        fake_now += 200000000ULL; // expire shared frame
        auto watch1 = analyzer.execute({{"operation", "watch"}, {"tracking_kind", "features"},
                                        {"context", ctx_base("r1", "c1", {40, 40, 560, 200})}});
        require_cond(watch1["status"] == "unchanged", "watch-unchanged-failed");

        scene.at(120, 100) ^= 0x7F;
        auto watch2 = analyzer.execute({{"operation", "watch"}, {"tracking_kind", "features"},
                                        {"context", ctx_base("r1", "c1", {40, 40, 560, 200})}});
        require_cond(watch2["status"] == "changed", "watch-changed-failed");
    }

    vision::json report = {
        {"passed", true},
        {"fft_ncc_parity", true},
        {"boundary_and_feature_parity", true},
        {"vtp1_roundtrip", true},
        {"analyzer_watch_and_shared_reuse", true},
    };
    std::ofstream out(output_dir / "result.json", std::ios::binary);
    out << report.dump(2);
    return 0;
}

int run_eval_json(const fs::path& input_path, const fs::path& output_path) {
    std::ifstream in(input_path, std::ios::binary);
    if (!in) throw std::runtime_error("cannot-open-eval-input");
    vision::json req = vision::json::parse(in);
    std::string op = req.at("op").get<std::string>();

    auto read_gray = [&](const char* key) {
        const auto& g = req.at(key);
        vision::GrayImage img;
        img.w = g.at("w").get<int>();
        img.h = g.at("h").get<int>();
        img.data = vision::hex_to_bytes(g.at("hex").get<std::string>(), size_t(img.w) * size_t(img.h) * 2 + 4);
        return img;
    };

    vision::json out = {{"ok", true}};
    try {
        if (op == "learn") {
            auto frame = read_gray("image");
            const auto& r = req.at("rect");
            vision::RectI rect{r[0].get<int>(), r[1].get<int>(), r[2].get<int>(), r[3].get<int>()};
            auto tpl = vision::learn_template(frame, rect, req.value("tracking_kind", std::string("boundary")));
            out["template_w"] = tpl.w;
            out["template_h"] = tpl.h;
            out["template_hex"] = vision::bytes_to_hex(tpl.data);
        } else if (op == "locate") {
            auto frame = read_gray("image");
            auto tpl = read_gray("template");
            auto res = vision::locate_template(frame, tpl, req.value("tracking_kind", std::string("boundary")));
            out["found"] = res.found;
            out["reason"] = res.reason;
            out["score"] = res.score;
            if (res.found) out["rect"] = {res.rect.x, res.rect.y, res.rect.w, res.rect.h};
            else out["rect"] = nullptr;
        } else if (op == "same_border_stroke") {
            auto frame = read_gray("image");
            const auto& a = req.at("first");
            const auto& b = req.at("second");
            out["same"] = vision::same_border_stroke(
                frame, {a[0].get<int>(), a[1].get<int>(), a[2].get<int>(), a[3].get<int>()},
                {b[0].get<int>(), b[1].get<int>(), b[2].get<int>(), b[3].get<int>()});
        } else if (op == "resize") {
            auto src = read_gray("image");
            auto dst = vision::resize_linear(src, req.at("dst_w").get<int>(), req.at("dst_h").get<int>());
            out["w"] = dst.w;
            out["h"] = dst.h;
            out["hex"] = vision::bytes_to_hex(dst.data);
        } else {
            throw std::runtime_error("unsupported-eval-op");
        }
    } catch (const std::exception& e) {
        out = {{"ok", false}, {"error", e.what()}};
    }

    std::ofstream os(output_path, std::ios::binary);
    os << out.dump();
    return 0;
}

} // namespace

int WINAPI wWinMain(HINSTANCE, HINSTANCE, PWSTR, int) {
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    if (!argv) return 2;

    std::wstring pipe_name, session_token, wake_name, self_test_dir, eval_in, eval_out;
    DWORD parent_pid = 0;
    bool has_parent = false;

    for (int i = 1; i < argc; ++i) {
        std::wstring arg = argv[i];
        auto next_val = [&](std::wstring& dst) -> bool {
            if (i + 1 >= argc) return false;
            dst = argv[++i];
            return true;
        };
        if (arg == L"--pipe") { if (!next_val(pipe_name)) { LocalFree(argv); return 2; } }
        else if (arg == L"--session") { if (!next_val(session_token)) { LocalFree(argv); return 2; } }
        else if (arg == L"--wake-event") { if (!next_val(wake_name)) { LocalFree(argv); return 2; } }
        else if (arg == L"--deps") { std::wstring ignored; if (!next_val(ignored)) { LocalFree(argv); return 2; } }
        else if (arg == L"--parent-pid") {
            std::wstring val;
            if (!next_val(val)) { LocalFree(argv); return 2; }
            try { parent_pid = DWORD(std::stoul(val)); has_parent = true; }
            catch (...) { LocalFree(argv); return 2; }
        } else if (arg == L"--self-test") {
            if (!next_val(self_test_dir)) { LocalFree(argv); return 2; }
        } else if (arg == L"--eval-json") {
            if (i + 2 >= argc) { LocalFree(argv); return 2; }
            eval_in = argv[++i];
            eval_out = argv[++i];
        } else {
            LocalFree(argv);
            return 2;
        }
    }
    LocalFree(argv);

    if (!self_test_dir.empty()) {
        try { return run_self_test(fs::path(self_test_dir)); }
        catch (...) { return 2; }
    }
    if (!eval_in.empty()) {
        try { return run_eval_json(fs::path(eval_in), fs::path(eval_out)); }
        catch (...) { return 2; }
    }

    if (pipe_name.empty() || session_token.empty() || !has_parent || parent_pid == 0)
        return 2;

    try { vision::dpi_init(); }
    catch (...) { return 2; }

    HANDLE parent = OpenProcess(SYNCHRONIZE, FALSE, parent_pid);
    if (!parent) return 2;

    HANDLE wake = nullptr;
    int exit_code = 0;
    try {
        PipeClient pipe(pipe_name, 2500);
        if (pipe.server_pid() != parent_pid) {
            CloseHandle(parent);
            return 2;
        }
        if (!wake_name.empty()) {
            wake = OpenEventW(SYNCHRONIZE, FALSE, wake_name.c_str());
            if (!wake) {
                CloseHandle(parent);
                return 2;
            }
        }

        auto narrow_utf8 = [](const std::wstring& w) -> std::string {
            if (w.empty()) return {};
            int len = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), int(w.size()), nullptr, 0, nullptr, nullptr);
            std::string s(size_t(len), '\0');
            WideCharToMultiByte(CP_UTF8, 0, w.c_str(), int(w.size()), s.data(), len, nullptr, nullptr);
            return s;
        };
        std::string session_utf8 = narrow_utf8(session_token);

        vision::Analyzer analyzer;
        vision::json response = nullptr;
        uint64_t request_id = 0;

        while (WaitForSingleObject(parent, 0) == WAIT_TIMEOUT) {
            analyzer.expire_shared();
            ++request_id;
            vision::json message = {
                {"protocol", kProtocol},
                {"session", session_utf8},
                {"request_id", request_id},
                {"op", request_id == 1 ? "hello" : "next"},
                {"result", response},
            };
            if (request_id == 1) {
                message["capabilities"] = {"multi_region_v1", "contained_roi_reuse_v1", "feature_position_v1"};
            }
            vision::json reply = pipe.exchange(message);
            if (!reply.value("ok", false) || reply.value("request_id", uint64_t(0)) != request_id) {
                exit_code = 2;
                break;
            }
            if (reply.value("stop", false)) {
                exit_code = 0;
                break;
            }
            if (reply.contains("task") && !reply["task"].is_null()) {
                const auto& task = reply["task"];
                auto t0 = std::chrono::steady_clock::now();
                try {
                    response = analyzer.execute(task);
                } catch (const std::exception& e) {
                    response = {{"status", "unavailable"}, {"reason", std::string("WorkerError:") + e.what()}};
                }
                double ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
                response["task_id"] = task.at("task_id");
                response["elapsed_ms"] = ms;
                response["cached_regions"] = int(analyzer.watches.size());
                response["retained_pixels"] = analyzer.retained_pixels();
            } else {
                response = nullptr;
                int wait_ms = std::clamp(reply.value("wait_ms", 100), 1, 1000);
                if (wake) {
                    HANDLE handles[2] = {parent, wake};
                    DWORD w = WaitForMultipleObjects(2, handles, FALSE, DWORD(wait_ms));
                    if (w == WAIT_OBJECT_0) { exit_code = 0; break; }
                    if (w != WAIT_OBJECT_0 + 1 && w != WAIT_TIMEOUT) { exit_code = 2; break; }
                } else {
                    if (WaitForSingleObject(parent, DWORD(wait_ms)) == WAIT_OBJECT_0) {
                        exit_code = 0;
                        break;
                    }
                }
            }
        }
    } catch (...) {
        exit_code = 2;
    }

    if (wake) CloseHandle(wake);
    CloseHandle(parent);
    return exit_code;
}
