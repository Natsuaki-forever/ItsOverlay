#pragma once
// Pure standard-C++17 visual tracking algorithms for OverlayVision.exe.
// Mirrors visual_tracking/vision.py (learn, locate, boundaries, same_border_stroke)
// without OpenCV, NumPy, or Python runtime dependencies.

#include <algorithm>
#include <cmath>
#include <complex>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

namespace vision {

constexpr int kMaxRoiPixels = 524288;
constexpr int kMaxTemplatePixels = 131072;
constexpr int kMaxActiveRegions = 4;
constexpr int kMaxRetainedPixels = 2097152;

struct RectI {
    int x = 0, y = 0, w = 0, h = 0;
    bool valid() const { return w > 0 && h > 0; }
};

struct GrayImage {
    int w = 0, h = 0;
    std::vector<uint8_t> data;

    bool empty() const { return w <= 0 || h <= 0 || data.empty(); }
    int size() const { return w * h; }
    uint8_t at(int x, int y) const { return data[size_t(y) * size_t(w) + size_t(x)]; }
    uint8_t& at(int x, int y) { return data[size_t(y) * size_t(w) + size_t(x)]; }

    GrayImage crop(int x, int y, int cw, int ch) const {
        if (x < 0 || y < 0 || cw <= 0 || ch <= 0 || x + cw > w || y + ch > h)
            throw std::runtime_error("crop-out-of-bounds");
        GrayImage out;
        out.w = cw;
        out.h = ch;
        out.data.resize(size_t(cw) * size_t(ch));
        for (int row = 0; row < ch; ++row) {
            const uint8_t* src = data.data() + size_t(y + row) * size_t(w) + size_t(x);
            std::copy(src, src + cw, out.data.data() + size_t(row) * size_t(cw));
        }
        return out;
    }

    bool equals(const GrayImage& other) const {
        return w == other.w && h == other.h && data == other.data;
    }
};

// Matches OpenCV cv2.cvtColor(BGR2GRAY) 14-bit fixed-point coefficients.
inline GrayImage bgrx_to_gray(const uint8_t* bgra, int w, int h, int stride_bytes = 0) {
    if (w < 1 || h < 1 || int64_t(w) * int64_t(h) > kMaxRoiPixels)
        throw std::runtime_error("roi-pixel-budget-exceeded");
    if (stride_bytes <= 0) stride_bytes = w * 4;
    GrayImage out;
    out.w = w;
    out.h = h;
    out.data.resize(size_t(w) * size_t(h));
    for (int y = 0; y < h; ++y) {
        const uint8_t* row = bgra + size_t(y) * size_t(stride_bytes);
        uint8_t* dst = out.data.data() + size_t(y) * size_t(w);
        for (int x = 0; x < w; ++x) {
            int b = row[x * 4 + 0];
            int g = row[x * 4 + 1];
            int r = row[x * 4 + 2];
            dst[x] = static_cast<uint8_t>((1868 * b + 9617 * g + 4899 * r + 8192) >> 14);
        }
    }
    return out;
}

inline double stddev_u8(const std::vector<uint8_t>& values) {
    if (values.empty()) return 0.0;
    double sum = 0.0, sq = 0.0;
    for (uint8_t v : values) {
        sum += double(v);
        sq += double(v) * double(v);
    }
    double n = double(values.size());
    double mean = sum / n;
    double var = std::max(0.0, sq / n - mean * mean);
    return std::sqrt(var);
}

inline double percentile_linear(std::vector<double> vals, double p) {
    if (vals.empty()) return 0.0;
    std::sort(vals.begin(), vals.end());
    double idx = (p / 100.0) * double(vals.size() - 1);
    size_t lo = static_cast<size_t>(std::floor(idx));
    size_t hi = static_cast<size_t>(std::ceil(idx));
    double frac = idx - double(lo);
    return vals[lo] * (1.0 - frac) + vals[hi] * frac;
}

// Matches OpenCV cv2.resize(..., INTER_LINEAR) pixel-center alignment.
inline GrayImage resize_linear(const GrayImage& src, int dst_w, int dst_h) {
    if (dst_w < 1 || dst_h < 1 || int64_t(dst_w) * int64_t(dst_h) > kMaxTemplatePixels)
        throw std::runtime_error("scaled-template-budget-exceeded");
    if (dst_w == src.w && dst_h == src.h) return src;
    GrayImage dst;
    dst.w = dst_w;
    dst.h = dst_h;
    dst.data.resize(size_t(dst_w) * size_t(dst_h));
    double scale_x = double(src.w) / double(dst_w);
    double scale_y = double(src.h) / double(dst_h);
    std::vector<int> x0(dst_w), x1(dst_w);
    std::vector<double> wx(dst_w);
    for (int x = 0; x < dst_w; ++x) {
        double fx = (x + 0.5) * scale_x - 0.5;
        int ix = static_cast<int>(std::floor(fx));
        double frac = fx - ix;
        if (ix < 0) { ix = 0; frac = 0.0; }
        if (ix >= src.w - 1) { ix = src.w - 1; frac = 0.0; }
        x0[x] = ix;
        x1[x] = std::min(ix + 1, src.w - 1);
        wx[x] = frac;
    }
    for (int y = 0; y < dst_h; ++y) {
        double fy = (y + 0.5) * scale_y - 0.5;
        int iy = static_cast<int>(std::floor(fy));
        double wy = fy - iy;
        if (iy < 0) { iy = 0; wy = 0.0; }
        if (iy >= src.h - 1) { iy = src.h - 1; wy = 0.0; }
        int iy1 = std::min(iy + 1, src.h - 1);
        const uint8_t* r0 = src.data.data() + size_t(iy) * size_t(src.w);
        const uint8_t* r1 = src.data.data() + size_t(iy1) * size_t(src.w);
        uint8_t* out_row = dst.data.data() + size_t(y) * size_t(dst_w);
        for (int x = 0; x < dst_w; ++x) {
            double top = r0[x0[x]] * (1.0 - wx[x]) + r0[x1[x]] * wx[x];
            double bot = r1[x0[x]] * (1.0 - wx[x]) + r1[x1[x]] * wx[x];
            int val = static_cast<int>(std::lround(top * (1.0 - wy) + bot * wy));
            out_row[x] = static_cast<uint8_t>(std::clamp(val, 0, 255));
        }
    }
    return dst;
}

// Matches OpenCV cv2.Canny(frame, low, high, apertureSize=3, L2gradient=false).
inline GrayImage canny_edges(const GrayImage& frame, int low_thresh, int high_thresh) {
    GrayImage out;
    out.w = frame.w;
    out.h = frame.h;
    out.data.assign(size_t(frame.w) * size_t(frame.h), 0);
    if (frame.w < 2 || frame.h < 2) return out;
    const int w = frame.w, h = frame.h;
    auto sample = [&](int x, int y) -> int {
        x = std::clamp(x, 0, w - 1);
        y = std::clamp(y, 0, h - 1);
        return int(frame.at(x, y));
    };
    std::vector<int> gx(size_t(w) * size_t(h), 0);
    std::vector<int> gy(size_t(w) * size_t(h), 0);
    std::vector<int> mag(size_t(w) * size_t(h), 0);
    for (int y = 0; y < h; ++y) {
        for (int x = 0; x < w; ++x) {
            int p00 = sample(x - 1, y - 1), p01 = sample(x, y - 1), p02 = sample(x + 1, y - 1);
            int p10 = sample(x - 1, y),     p12 = sample(x + 1, y);
            int p20 = sample(x - 1, y + 1), p21 = sample(x, y + 1), p22 = sample(x + 1, y + 1);
            int dx = (p02 + 2 * p12 + p22) - (p00 + 2 * p10 + p20);
            int dy = (p20 + 2 * p21 + p22) - (p00 + 2 * p01 + p02);
            size_t idx = size_t(y) * size_t(w) + size_t(x);
            gx[idx] = dx;
            gy[idx] = dy;
            mag[idx] = std::abs(dx) + std::abs(dy);
        }
    }
    // 0 = suppressed, 1 = weak (> low), 2 = strong (> high)
    std::vector<uint8_t> state(size_t(w) * size_t(h), 0);
    std::vector<int> stack;
    stack.reserve(size_t(w) * size_t(h) / 16 + 64);

    auto mag_at = [&](int x, int y) -> int {
        if (x < 0 || x >= w || y < 0 || y >= h) return 0;
        return mag[size_t(y) * size_t(w) + size_t(x)];
    };

    // OpenCV fixed-point tan(22.5 deg) (~13573 / 32768) and tan(67.5 deg) (~79109 / 32768).
    for (int y = 0; y < h; ++y) {
        for (int x = 0; x < w; ++x) {
            size_t idx = size_t(y) * size_t(w) + size_t(x);
            int m = mag[idx];
            if (m <= low_thresh) continue;
            int dx = gx[idx], dy = gy[idx];
            int64_t adx = std::abs(dx), ady = std::abs(dy);
            int64_t t22 = adx * 13573;
            int64_t t67 = adx * 79109;
            int64_t sdy = ady << 15;
            bool keep = false;
            if (sdy < t22) {
                keep = (m > mag_at(x - 1, y)) && (m >= mag_at(x + 1, y));
            } else if (sdy > t67) {
                keep = (m > mag_at(x, y - 1)) && (m >= mag_at(x, y + 1));
            } else {
                int s = (dx ^ dy) < 0 ? -1 : 1;
                keep = (m > mag_at(x - s, y - 1)) && (m > mag_at(x + s, y + 1));
            }
            if (!keep) continue;
            if (m > high_thresh) {
                state[idx] = 2;
                stack.push_back(int(idx));
            } else {
                state[idx] = 1;
            }
        }
    }

    static constexpr int kDx8[8] = {-1, 0, 1, -1, 1, -1, 0, 1};
    static constexpr int kDy8[8] = {-1, -1, -1, 0, 0, 1, 1, 1};
    while (!stack.empty()) {
        int cur = stack.back();
        stack.pop_back();
        int cx = cur % w, cy = cur / w;
        for (int k = 0; k < 8; ++k) {
            int nx = cx + kDx8[k], ny = cy + kDy8[k];
            if (nx < 0 || nx >= w || ny < 0 || ny >= h) continue;
            size_t nidx = size_t(ny) * size_t(w) + size_t(nx);
            if (state[nidx] == 1) {
                state[nidx] = 2;
                stack.push_back(int(nidx));
            }
        }
    }
    for (size_t i = 0; i < state.size(); ++i)
        out.data[i] = (state[i] == 2) ? 255 : 0;
    return out;
}

inline int count_nonzero(const GrayImage& img) {
    int count = 0;
    for (uint8_t v : img.data) if (v) ++count;
    return count;
}

struct ComponentStat {
    int left = 0, top = 0, right = 0, bottom = 0, area = 0;
    int width() const { return right - left + 1; }
    int height() const { return bottom - top + 1; }
};

// 8-connected components on binary image (non-zero = foreground).
inline int connected_components_8(const GrayImage& mask, std::vector<int>& labels,
                                  std::vector<ComponentStat>& stats, int max_components) {
    const int w = mask.w, h = mask.h;
    labels.assign(size_t(w) * size_t(h), 0);
    stats.clear();
    stats.push_back({}); // label 0 = background
    std::vector<int> q;
    static constexpr int kDx8[8] = {-1, 0, 1, -1, 1, -1, 0, 1};
    static constexpr int kDy8[8] = {-1, -1, -1, 0, 0, 1, 1, 1};
    int next_label = 0;
    for (int y = 0; y < h; ++y) {
        for (int x = 0; x < w; ++x) {
            size_t start = size_t(y) * size_t(w) + size_t(x);
            if (!mask.data[start] || labels[start] != 0) continue;
            ++next_label;
            if (next_label > max_components) return next_label;
            ComponentStat st{x, y, x, y, 0};
            labels[start] = next_label;
            q.clear();
            q.push_back(int(start));
            size_t head = 0;
            while (head < q.size()) {
                int cur = q[head++];
                int cx = cur % w, cy = cur / w;
                ++st.area;
                if (cx < st.left) st.left = cx;
                if (cx > st.right) st.right = cx;
                if (cy < st.top) st.top = cy;
                if (cy > st.bottom) st.bottom = cy;
                for (int k = 0; k < 8; ++k) {
                    int nx = cx + kDx8[k], ny = cy + kDy8[k];
                    if (nx < 0 || nx >= w || ny < 0 || ny >= h) continue;
                    size_t nidx = size_t(ny) * size_t(w) + size_t(nx);
                    if (mask.data[nidx] && labels[nidx] == 0) {
                        labels[nidx] = next_label;
                        q.push_back(int(nidx));
                    }
                }
            }
            stats.push_back(st);
        }
    }
    return next_label;
}

inline RectI checked_selection(const RectI& rect, int width, int height) {
    if (rect.x < 0 || rect.y < 0 || rect.w < 12 || rect.h < 12 ||
        rect.x + rect.w > width || rect.y + rect.h > height)
        throw std::runtime_error("selection-outside-image");
    return rect;
}

// Mirrors visual_tracking/vision.py:learn()
inline GrayImage learn_template(const GrayImage& frame, const RectI& rect, const std::string& tracking_kind) {
    if (frame.empty() || frame.size() > kMaxRoiPixels)
        throw std::runtime_error("roi-pixel-budget-exceeded");
    RectI r = checked_selection(rect, frame.w, frame.h);
    if (tracking_kind == "features") {
        if (int64_t(r.w) * int64_t(r.h) > kMaxTemplatePixels)
            throw std::runtime_error("template-pixel-budget-exceeded");
        GrayImage tpl = frame.crop(r.x, r.y, r.w, r.h);
        if (stddev_u8(tpl.data) < 4.0 || count_nonzero(canny_edges(tpl, 30, 90)) < 12)
            throw std::runtime_error("insufficient-stable-features");
        return tpl;
    }
    if (tracking_kind != "boundary")
        throw std::runtime_error("invalid-tracking-kind");
    if (r.w <= 10 || r.h <= 10)
        throw std::runtime_error("selection-outside-image");
    GrayImage crop = frame.crop(r.x + 5, r.y + 5, r.w - 10, r.h - 10);
    if (crop.w < 6 || crop.h < 6)
        throw std::runtime_error("insufficient-stable-features");

    std::vector<double> perimeter;
    perimeter.reserve(size_t(crop.w) * 6 + size_t(crop.h) * 6);
    for (int y = 0; y < 3; ++y)
        for (int x = 0; x < crop.w; ++x) perimeter.push_back(double(crop.at(x, y)));
    for (int y = crop.h - 3; y < crop.h; ++y)
        for (int x = 0; x < crop.w; ++x) perimeter.push_back(double(crop.at(x, y)));
    for (int y = 0; y < crop.h; ++y)
        for (int x = 0; x < 3; ++x) perimeter.push_back(double(crop.at(x, y)));
    for (int y = 0; y < crop.h; ++y)
        for (int x = crop.w - 3; x < crop.w; ++x) perimeter.push_back(double(crop.at(x, y)));

    double background = percentile_linear(perimeter, 50.0);
    std::vector<double> perim_delta;
    perim_delta.reserve(perimeter.size());
    for (double v : perimeter) perim_delta.push_back(std::abs(v - background));
    double p90 = percentile_linear(perim_delta, 90.0);
    if (p90 > 12.0) throw std::runtime_error("background-not-stable");

    double threshold = std::max(24.0, p90 * 3.0);
    GrayImage mask;
    mask.w = crop.w;
    mask.h = crop.h;
    mask.data.resize(crop.data.size());
    for (size_t i = 0; i < crop.data.size(); ++i)
        mask.data[i] = (std::abs(double(crop.data[i]) - background) >= threshold) ? 1 : 0;

    int corner = std::max(1, int(std::min(crop.w, crop.h) * 0.35));
    for (int y = 0; y < corner; ++y) {
        for (int x = 0; x < corner; ++x) {
            mask.at(x, y) = 0;
            mask.at(crop.w - 1 - x, y) = 0;
            mask.at(x, crop.h - 1 - y) = 0;
            mask.at(crop.w - 1 - x, crop.h - 1 - y) = 0;
        }
    }
    int mask_count = count_nonzero(mask);
    if (double(mask_count) / double(crop.size()) > 0.3)
        throw std::runtime_error("features-too-dense");

    std::vector<int> labels;
    std::vector<ComponentStat> stats;
    int num_components = connected_components_8(mask, labels, stats, 511);
    if (num_components + 1 > 512)
        throw std::runtime_error("features-too-complex");

    int min_x = crop.w, min_y = crop.h, max_x = -1, max_y = -1;
    int selected_pixels = 0;
    std::vector<uint8_t> keep_label(stats.size(), 0);
    for (int i = 1; i < int(stats.size()); ++i) {
        const auto& st = stats[i];
        int cw = st.width(), ch = st.height();
        if (st.area < 2 || double(st.area) > double(crop.size()) * 0.15 ||
            st.left == 0 || st.top == 0 ||
            st.right + 1 == crop.w || st.bottom + 1 == crop.h ||
            double(cw) > double(crop.w) * 0.8 || double(ch) > double(crop.h) * 0.8)
            continue;
        keep_label[i] = 1;
    }
    for (int y = 0; y < crop.h; ++y) {
        for (int x = 0; x < crop.w; ++x) {
            int lbl = labels[size_t(y) * size_t(crop.w) + size_t(x)];
            if (lbl > 0 && keep_label[lbl]) {
                ++selected_pixels;
                if (x < min_x) min_x = x;
                if (x > max_x) max_x = x;
                if (y < min_y) min_y = y;
                if (y > max_y) max_y = y;
            }
        }
    }
    if (selected_pixels < 25 || double(selected_pixels) > double(crop.size()) * 0.3)
        throw std::runtime_error("insufficient-stable-features");

    int a = std::max(0, min_x - 3);
    int b = std::max(0, min_y - 3);
    int c = std::min(crop.w, max_x + 4);
    int d = std::min(crop.h, max_y + 4);
    GrayImage tpl = crop.crop(a, b, c - a, d - b);
    if (tpl.size() > kMaxTemplatePixels)
        throw std::runtime_error("template-pixel-budget-exceeded");
    if (stddev_u8(tpl.data) < 4.0)
        throw std::runtime_error("template-low-variance");
    return tpl;
}

// Mirrors visual_tracking/vision.py:same_border_stroke()
inline bool same_border_stroke(const GrayImage& frame, const RectI& first, const RectI& second) {
    int ax = first.x, ay = first.y, aw = first.w, ah = first.h;
    int bx = second.x, by = second.y, bw = second.w, bh = second.h;
    if (std::max({std::abs(ax - bx), std::abs(ay - by),
                  std::abs(ax + aw - bx - bw), std::abs(ay + ah - by - bh)}) > 6)
        return false;
    int left = std::max(ax, bx), top = std::max(ay, by);
    int right = std::min(ax + aw, bx + bw), bottom = std::min(ay + ah, by + bh);
    if (right - left < 20 || bottom - top < 20) return false;
    std::vector<double> interior;
    interior.reserve(size_t(bottom - top - 10) * size_t(right - left - 10));
    for (int y = top + 5; y < bottom - 5; ++y)
        for (int x = left + 5; x < right - 5; ++x)
            interior.push_back(double(frame.at(x, y)));
    double background = percentile_linear(interior, 50.0);
    int inset = std::max(5, int(std::min(right - left, bottom - top) * 0.35));

    auto band_ok = [&](int x0, int y0, int x1, int y1) -> bool {
        x0 = std::clamp(x0, 0, frame.w); x1 = std::clamp(x1, 0, frame.w);
        y0 = std::clamp(y0, 0, frame.h); y1 = std::clamp(y1, 0, frame.h);
        if (x1 <= x0 || y1 <= y0) return true;
        int total = 0, contrast = 0;
        for (int y = y0; y < y1; ++y) {
            for (int x = x0; x < x1; ++x) {
                ++total;
                if (std::abs(double(frame.at(x, y)) - background) > 16.0) ++contrast;
            }
        }
        return total == 0 || (double(contrast) / double(total)) >= 0.95;
    };

    return band_ok(std::min(ax, bx) + 1, top + inset, std::max(ax, bx) + 1, bottom - inset) &&
           band_ok(std::min(ax + aw, bx + bw), top + inset, std::max(ax + aw, bx + bw), bottom - inset) &&
           band_ok(left + inset, std::min(ay, by) + 1, right - inset, std::max(ay, by) + 1) &&
           band_ok(left + inset, std::min(ay + ah, by + bh), right - inset, std::max(ay + ah, by + bh));
}

// Compute enclosed area (interior + border pixels) of a single connected edge component
// by 4-connected exterior flood fill inside a 1-pixel padded bounding box.
inline int enclosed_area_of_component(const std::vector<int>& labels, int full_w,
                                      int label_id, const ComponentStat& st) {
    int cw = st.width(), ch = st.height();
    int pw = cw + 2, ph = ch + 2;
    std::vector<uint8_t> visited(size_t(pw) * size_t(ph), 0);
    std::vector<int> q;
    q.reserve(size_t(pw) * size_t(ph) / 4 + 64);
    visited[0] = 1;
    q.push_back(0);
    size_t head = 0;
    int outside_inside_box = 0;
    static constexpr int kDx4[4] = {-1, 1, 0, 0};
    static constexpr int kDy4[4] = {0, 0, -1, 1};
    while (head < q.size()) {
        int cur = q[head++];
        int px = cur % pw, py = cur / pw;
        if (px >= 1 && px <= cw && py >= 1 && py <= ch) ++outside_inside_box;
        for (int k = 0; k < 4; ++k) {
            int nx = px + kDx4[k], ny = py + kDy4[k];
            if (nx < 0 || nx >= pw || ny < 0 || ny >= ph) continue;
            size_t nidx = size_t(ny) * size_t(pw) + size_t(nx);
            if (visited[nidx]) continue;
            if (nx >= 1 && nx <= cw && ny >= 1 && ny <= ch) {
                int gx = st.left + nx - 1, gy = st.top + ny - 1;
                if (labels[size_t(gy) * size_t(full_w) + size_t(gx)] == label_id) continue;
            }
            visited[nidx] = 1;
            q.push_back(int(nidx));
        }
    }
    return cw * ch - outside_inside_box;
}

// Mirrors visual_tracking/vision.py:boundaries()
inline std::vector<RectI> find_boundaries(const GrayImage& frame) {
    std::vector<RectI> boxes;
    const int kPairs[2][2] = {{45, 120}, {12, 36}};
    std::vector<int> labels;
    std::vector<ComponentStat> stats;
    for (const auto& pair : kPairs) {
        GrayImage edges = canny_edges(frame, pair[0], pair[1]);
        int count = connected_components_8(edges, labels, stats, 4096);
        if (count > 4096) throw std::runtime_error("boundaries-too-complex");
        for (int i = 1; i <= count; ++i) {
            const auto& st = stats[i];
            int x = st.left, y = st.top, w = st.width(), h = st.height();
            if (w < 140 || h < 55 || double(w) * double(h) > double(frame.size()) * 0.999)
                continue;
            int inset = std::max(3, int(std::min(w, h) * 0.3));
            if (w - 2 * inset <= 0 || h - 2 * inset <= 0) continue;

            auto check_horizontal = [&](int y0) -> bool {
                int len = w - 2 * inset;
                int hits = 0, cur_gap = 0, max_gap = 0;
                for (int dx = 0; dx < len; ++dx) {
                    int gx = x + inset + dx;
                    bool on = false;
                    for (int dy = 0; dy < 3; ++dy) {
                        if (edges.at(gx, y0 + dy)) { on = true; break; }
                    }
                    if (on) { ++hits; cur_gap = 0; }
                    else { ++cur_gap; if (cur_gap > max_gap) max_gap = cur_gap; }
                }
                return double(hits) / double(len) >= 0.86 && max_gap <= 3;
            };
            auto check_vertical = [&](int x0) -> bool {
                int len = h - 2 * inset;
                int hits = 0, cur_gap = 0, max_gap = 0;
                for (int dy = 0; dy < len; ++dy) {
                    int gy = y + inset + dy;
                    bool on = false;
                    for (int dx = 0; dx < 3; ++dx) {
                        if (edges.at(x0 + dx, gy)) { on = true; break; }
                    }
                    if (on) { ++hits; cur_gap = 0; }
                    else { ++cur_gap; if (cur_gap > max_gap) max_gap = cur_gap; }
                }
                return double(hits) / double(len) >= 0.86 && max_gap <= 3;
            };

            if (!check_horizontal(y) || !check_horizontal(y + h - 3) ||
                !check_vertical(x) || !check_vertical(x + w - 3))
                continue;

            int enclosed = enclosed_area_of_component(labels, edges.w, i, st);
            if (double(enclosed) < double(w) * double(h) * 0.75) continue;

            RectI candidate{x, y, w, h};
            int dup = -1;
            for (int b = 0; b < int(boxes.size()); ++b) {
                const auto& e = boxes[b];
                if (std::max({std::abs(x - e.x), std::abs(y - e.y),
                              std::abs(x + w - e.x - e.w), std::abs(y + h - e.y - e.h)}) <= 3 ||
                    same_border_stroke(frame, candidate, e)) {
                    dup = b;
                    break;
                }
            }
            if (dup < 0) boxes.push_back(candidate);
            else if (w * h > boxes[dup].w * boxes[dup].h) boxes[dup] = candidate;
        }
    }
    return boxes;
}

// Iterative radix-2 Cooley-Tukey 1D FFT (in-place, float precision for cache & SIMD efficiency).
inline void fft1d(std::vector<std::complex<float>>& a, bool inverse) {
    const size_t n = a.size();
    for (size_t i = 1, j = 0; i < n; ++i) {
        size_t bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap(a[i], a[j]);
    }
    constexpr double kPi = 3.14159265358979323846;
    for (size_t len = 2; len <= n; len <<= 1) {
        double ang = (inverse ? 2.0 : -2.0) * kPi / double(len);
        std::complex<float> wlen(float(std::cos(ang)), float(std::sin(ang)));
        for (size_t i = 0; i < n; i += len) {
            std::complex<float> w(1.0f, 0.0f);
            size_t half = len >> 1;
            for (size_t j = 0; j < half; ++j) {
                auto u = a[i + j];
                auto v = a[i + j + half] * w;
                a[i + j] = u + v;
                a[i + j + half] = u - v;
                w *= wlen;
            }
        }
    }
    if (inverse) {
        float inv = 1.0f / float(n);
        for (auto& x : a) x *= inv;
    }
}

inline void fft2d(std::vector<std::complex<float>>& grid, int w, int h, bool inverse) {
    std::vector<std::complex<float>> buf(size_t(std::max(w, h)));
    for (int y = 0; y < h; ++y) {
        buf.assign(grid.begin() + size_t(y) * size_t(w), grid.begin() + size_t(y + 1) * size_t(w));
        fft1d(buf, inverse);
        std::copy(buf.begin(), buf.end(), grid.begin() + size_t(y) * size_t(w));
    }
    buf.resize(size_t(h));
    for (int x = 0; x < w; ++x) {
        for (int y = 0; y < h; ++y) buf[y] = grid[size_t(y) * size_t(w) + size_t(x)];
        fft1d(buf, inverse);
        for (int y = 0; y < h; ++y) grid[size_t(y) * size_t(w) + size_t(x)] = buf[y];
    }
}

inline int next_pow2(int v) {
    int p = 1;
    while (p < v) p <<= 1;
    return p;
}

// Exact TM_CCOEFF_NORMED score map (width = fw-tw+1, height = fh-th+1).
// Uses exact int64 integral images for window variance and 2D FFT for large cross-correlations.
inline std::vector<double> match_template_ccoeff_normed(const GrayImage& frame, const GrayImage& tpl,
                                                        int& out_w, int& out_h) {
    const int fw = frame.w, fh = frame.h, tw = tpl.w, th = tpl.h;
    out_w = fw - tw + 1;
    out_h = fh - th + 1;
    if (out_w <= 0 || out_h <= 0) return {};

    const int area = tw * th;
    double t_sum = 0.0;
    for (uint8_t v : tpl.data) t_sum += double(v);
    double t_mean = t_sum / double(area);
    std::vector<double> t_zero(static_cast<size_t>(area));
    double t_var = 0.0;
    for (int i = 0; i < area; ++i) {
        double d = double(tpl.data[i]) - t_mean;
        t_zero[i] = d;
        t_var += d * d;
    }

    const int iw = fw + 1, ih = fh + 1;
    std::vector<int64_t> sum_img(size_t(iw) * size_t(ih), 0);
    std::vector<int64_t> sq_img(size_t(iw) * size_t(ih), 0);
    for (int y = 0; y < fh; ++y) {
        int64_t row_s = 0, row_sq = 0;
        const uint8_t* src = frame.data.data() + size_t(y) * size_t(fw);
        int64_t* prev_s = sum_img.data() + size_t(y) * size_t(iw);
        int64_t* cur_s = sum_img.data() + size_t(y + 1) * size_t(iw);
        int64_t* prev_sq = sq_img.data() + size_t(y) * size_t(iw);
        int64_t* cur_sq = sq_img.data() + size_t(y + 1) * size_t(iw);
        for (int x = 0; x < fw; ++x) {
            int64_t v = src[x];
            row_s += v;
            row_sq += v * v;
            cur_s[x + 1] = prev_s[x + 1] + row_s;
            cur_sq[x + 1] = prev_sq[x + 1] + row_sq;
        }
    }

    std::vector<double> scores(size_t(out_w) * size_t(out_h), 0.0);
    if (t_var <= 1e-12) return scores;

    auto compute_denom = [&](int x, int y) -> double {
        int x2 = x + tw, y2 = y + th;
        int64_t s = sum_img[size_t(y2) * iw + x2] - sum_img[size_t(y) * iw + x2]
                  - sum_img[size_t(y2) * iw + x]  + sum_img[size_t(y) * iw + x];
        int64_t sq = sq_img[size_t(y2) * iw + x2] - sq_img[size_t(y) * iw + x2]
                   - sq_img[size_t(y2) * iw + x]  + sq_img[size_t(y) * iw + x];
        double i_var = std::max(0.0, double(sq) - (double(s) * double(s)) / double(area));
        if (i_var <= 1e-12) return 0.0;
        return std::sqrt(i_var * t_var);
    };

    const int64_t direct_ops = int64_t(out_w) * int64_t(out_h) * int64_t(area);
    if (direct_ops <= 2500000LL) {
        for (int y = 0; y < out_h; ++y) {
            for (int x = 0; x < out_w; ++x) {
                double denom = compute_denom(x, y);
                if (denom <= 1e-12) continue;
                double num = 0.0;
                for (int ty = 0; ty < th; ++ty) {
                    const uint8_t* frow = frame.data.data() + size_t(y + ty) * size_t(fw) + size_t(x);
                    const double* trow = t_zero.data() + size_t(ty) * size_t(tw);
                    for (int tx = 0; tx < tw; ++tx) num += double(frow[tx]) * trow[tx];
                }
                scores[size_t(y) * size_t(out_w) + size_t(x)] = std::clamp(num / denom, -1.0, 1.0);
            }
        }
        return scores;
    }

    const int pw = next_pow2(fw);
    const int ph = next_pow2(fh);
    std::vector<std::complex<float>> f_fft(size_t(pw) * size_t(ph), {0.0f, 0.0f});
    std::vector<std::complex<float>> t_fft(size_t(pw) * size_t(ph), {0.0f, 0.0f});
    for (int y = 0; y < fh; ++y) {
        const uint8_t* frow = frame.data.data() + size_t(y) * size_t(fw);
        auto* dst = f_fft.data() + size_t(y) * size_t(pw);
        for (int x = 0; x < fw; ++x) dst[x] = {float(frow[x]), 0.0f};
    }
    for (int y = 0; y < th; ++y) {
        const double* trow = t_zero.data() + size_t(th - 1 - y) * size_t(tw);
        auto* dst = t_fft.data() + size_t(y) * size_t(pw);
        for (int x = 0; x < tw; ++x) dst[x] = {float(trow[tw - 1 - x]), 0.0f};
    }
    fft2d(f_fft, pw, ph, false);
    fft2d(t_fft, pw, ph, false);
    for (size_t i = 0; i < f_fft.size(); ++i) f_fft[i] *= t_fft[i];
    fft2d(f_fft, pw, ph, true);

    for (int y = 0; y < out_h; ++y) {
        const auto* crow = f_fft.data() + size_t(y + th - 1) * size_t(pw);
        double* srow = scores.data() + size_t(y) * size_t(out_w);
        for (int x = 0; x < out_w; ++x) {
            double denom = compute_denom(x, y);
            if (denom <= 1e-12) continue;
            srow[x] = std::clamp(double(crow[x + tw - 1].real()) / denom, -1.0, 1.0);
        }
    }
    return scores;
}

struct LocateResult {
    bool found = false;
    RectI rect{};
    std::string reason;
    double score = 0.0;
};

// Mirrors visual_tracking/vision.py:locate()
inline LocateResult locate_template(const GrayImage& frame, const GrayImage& tpl, const std::string& tracking_kind) {
    if (frame.empty() || frame.size() > kMaxRoiPixels)
        throw std::runtime_error("roi-pixel-budget-exceeded");
    if (tpl.empty() || tpl.size() > kMaxTemplatePixels || stddev_u8(tpl.data) < 4.0)
        throw std::runtime_error("invalid-template");
    if (tpl.h > frame.h || tpl.w > frame.w)
        return {false, {}, "template-too-large", 0.0};

    int sw = 0, sh = 0;
    std::vector<double> scores = match_template_ccoeff_normed(frame, tpl, sw, sh);
    double best = -2.0;
    int tx = 0, ty = 0;
    for (int y = 0; y < sh; ++y) {
        const double* row = scores.data() + size_t(y) * size_t(sw);
        for (int x = 0; x < sw; ++x) {
            if (std::isfinite(row[x]) && row[x] > best) {
                best = row[x];
                tx = x;
                ty = y;
            }
        }
    }
    if (!std::isfinite(best) || best < 0.84)
        return {false, {}, "features-lost", std::isfinite(best) && best > -1.5 ? best : 0.0};

    int sy0 = std::max(0, ty - tpl.h / 2), sy1 = std::min(sh, ty + tpl.h / 2 + 1);
    int sx0 = std::max(0, tx - tpl.w / 2), sx1 = std::min(sw, tx + tpl.w / 2 + 1);
    for (int y = sy0; y < sy1; ++y)
        for (int x = sx0; x < sx1; ++x)
            scores[size_t(y) * size_t(sw) + size_t(x)] = -1.0;

    double second = -2.0;
    for (double v : scores) if (std::isfinite(v) && v > second) second = v;
    if (second >= 0.84 || best - second < 0.08)
        return {false, {}, "ambiguous-features", best};

    if (tracking_kind == "features")
        return {true, {tx, ty, tpl.w, tpl.h}, "unique-feature-position", best};
    if (tracking_kind != "boundary")
        throw std::runtime_error("invalid-tracking-kind");

    std::vector<RectI> all_boxes = find_boundaries(frame);
    std::vector<RectI> enclosing;
    for (const auto& b : all_boxes) {
        if (b.x <= tx && b.y <= ty && b.x + b.w >= tx + tpl.w && b.y + b.h >= ty + tpl.h)
            enclosing.push_back(b);
    }
    if (enclosing.size() != 1)
        return {false, {}, "boundary-unobservable-or-ambiguous", best};

    const auto& b = enclosing[0];
    int x0 = std::max(0, b.x - 2);
    int y0 = std::max(0, b.y - 2);
    int x1 = std::min(frame.w, b.x + b.w + 2);
    int y1 = std::min(frame.h, b.y + b.h + 2);
    return {true, {x0, y0, x1 - x0, y1 - y0}, "snapshot-supported", best};
}

} // namespace vision
