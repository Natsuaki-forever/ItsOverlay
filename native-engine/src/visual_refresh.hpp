#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>

// Scheduling and presentation are independent. A failed check never changes
// the last confirmed client-relative rectangle.
struct VisualRefreshSchedule {
    bool requested=true,active=false,probe=false;
    double readyAt=0,nextAt=0,deadline=0;
    // Full refresh (geometry/DPI change, foreground resume, manual retry).
    // Always supersedes a pending lightweight probe.
    void request(double now,double settle=.15){
        requested=true;active=false;probe=false;readyAt=now+settle;
    }
    // Lightweight watch probe triggered after user input settles. Never
    // downgrades a pending full refresh or interrupts an active round.
    bool request_probe(double now,double settle=.25){
        if(active||(requested&&!probe))return false;
        requested=true;probe=true;readyAt=now+settle;return true;
    }
    int wait_ms(double now)const{
        if(active||(requested&&now>=readyAt))return 50;
        return requested?std::clamp(int(std::ceil((readyAt-now)*1000)),1,1000):1000;
    }
    bool due(double now,int interval)const{
        return active||(requested?now>=readyAt:interval>0&&now>=nextAt);
    }
    void start(double now){
        bool isProbe=requested&&probe;
        active=true;requested=false;probe=isProbe;deadline=now+8;
    }
    // Upgrade a lightweight probe round to full locate/search after pixel change.
    void promote_to_full(){probe=false;}
    void finish(double now,int interval){
        active=requested=probe=false;nextAt=interval>0?now+interval:0;
    }
};

// Pure-state debounce/cooldown tracker driven by GetLastInputInfo().
// Observes only the OS idle timestamp (never keycodes or hooks) so it covers
// clicks, wheel scrolling, touchpads, and keyboard shortcuts without AV risk.
struct VisualInputProbe {
    static constexpr double kSettleSeconds=.25;
    static constexpr double kCooldownSeconds=.80;
    bool initialized=false,pending=false;
    uint32_t lastInputTick=0;
    double inputSeenAt=0,lastTriggeredAt=-1e9;

    void reset(){initialized=false;pending=false;}
    bool observe(uint32_t inputTick,double now){
        if(!initialized){
            initialized=true;lastInputTick=inputTick;pending=false;return false;
        }
        if(inputTick!=lastInputTick){
            lastInputTick=inputTick;inputSeenAt=now;pending=true;return false;
        }
        if(pending&&now-inputSeenAt>=kSettleSeconds&&now-lastTriggeredAt>=kCooldownSeconds){
            pending=false;lastTriggeredAt=now;return true;
        }
        return false;
    }
    int wait_ms(double now)const{
        if(!pending)return 200;
        double target=std::max(inputSeenAt+kSettleSeconds,lastTriggeredAt+kCooldownSeconds);
        return std::clamp(int(std::ceil((target-now)*1000)),50,200);
    }
};
