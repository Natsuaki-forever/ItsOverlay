#pragma once
#include "visual_search.hpp"
#include "visual_tracking_state.hpp"
#include "visual_refresh.hpp"
#include "pipe.hpp"

inline void visual_search_tests(const fs::path& output){
    fs::create_directories(output);
    auto wakeName=L"Local\\ChatGPTOverlay.VisualWake.Test."+wide(new_id());
    WinHandle wake(create_user_wake_event(wakeName));WinHandle listener(OpenEventW(SYNCHRONIZE,FALSE,wakeName.c_str()));
    require(listener.value&&WaitForSingleObject(listener.value,0)==WAIT_TIMEOUT,"wake event unavailable or initially signaled");
    SetEvent(wake.value);require(WaitForSingleObject(listener.value,0)==WAIT_OBJECT_0&&WaitForSingleObject(listener.value,0)==WAIT_TIMEOUT,"wake event does not coalesce and auto-reset");
    bool collision=false;try{WinHandle duplicate(create_user_wake_event(wakeName));}catch(...){collision=true;}
    require(collision,"wake event silently reused a pre-existing object");
    VisualRefreshSchedule schedule;
    require(schedule.due(1,15),"first refresh not scheduled");schedule.start(1);
    require(schedule.active&&schedule.deadline==9,"refresh round lacks time budget");schedule.finish(2,15);
    require(!schedule.due(16.99,15)&&schedule.due(17,15),"idle refresh is not fifteen seconds");
    schedule.finish(2,0);require(!schedule.due(1000,0),"disabled periodic refresh still runs");
    schedule.request(1000);require(!schedule.due(1000.14,0)&&schedule.due(1000.15,0),"geometry settle ignored");
    require(schedule.wait_ms(1000)>=149&&schedule.wait_ms(1000)<=151,"requested refresh retained one-second sleep");
    schedule.request(1000.1);require(!schedule.due(1000.15,15),"continuous resize did not debounce");
    // Lightweight input probe scheduling must never downgrade full refreshes.
    require(!schedule.request_probe(1000.12,0),"probe downgraded pending full refresh");
    require(!schedule.probe,"full refresh lost priority");
    schedule.finish(1001,15);
    require(schedule.request_probe(1002,.25)&&schedule.probe,"idle probe request failed");
    require(!schedule.due(1002.24,15)&&schedule.due(1002.25,15),"probe settle ignored");
    schedule.start(1002.25);require(schedule.active&&schedule.probe&&schedule.deadline==1010.25,"started probe lost mode or budget");
    require(!schedule.request_probe(1002.5,0),"probe interrupted active round");
    schedule.promote_to_full();require(schedule.active&&!schedule.probe&&schedule.deadline==1010.25,"probe promotion dropped budget");
    schedule.finish(1003,15);
    require(schedule.due(1018,15),"fallback schedule broken after probe");
    schedule.start(1018);require(!schedule.probe,"periodic fallback started as lightweight probe");
    // Input idle tracker: baseline, debounce, cooldown, and focus reset.
    VisualInputProbe inputProbe;
    require(!inputProbe.observe(100,10),"first input sample falsely triggered probe");
    require(!inputProbe.observe(100,15),"unchanged input timestamp triggered probe");
    require(!inputProbe.observe(200,16.0),"initial input triggered before settle");
    require(!inputProbe.observe(200,16.24),"input triggered before 250ms settle");
    require(!inputProbe.observe(300,16.25),"continuous input did not extend settle");
    require(!inputProbe.observe(300,16.49),"extended settle fired early");
    require(inputProbe.observe(300,16.50),"settled input failed to trigger probe");
    require(!inputProbe.observe(300,17.0),"settled probe fired twice without new input");
    require(!inputProbe.observe(400,16.75),"new input during cooldown fired early");
    require(!inputProbe.observe(400,17.25),"cooldown was not enforced");
    require(inputProbe.observe(400,17.31),"post-cooldown probe did not fire");
    require(!inputProbe.observe(500,18.0),"input before reset unexpectedly fired");
    inputProbe.reset();
    require(!inputProbe.observe(500,19.0),"reset did not drop pending input");
    VisualTrackingState temporary;temporary.bind_session(555);temporary.calibrate({100,100,180,45},168);
    temporary.retryTemporaryLoss=true;temporary.permit(1);temporary.tick(1.4,true);
    for(int i=0;i<4;i++){
        double now=2+i*.2;auto t=temporary.begin(now,true,false,true);
        require(temporary.observe(t,i+1,now,now,VisualObservation::NotFound,{},true),"temporary missing sample rejected");
    }
    require(!temporary.locked(),"temporary boundary loss permanently required reselection");
    require(feature_scaled_size(123,1.5)==184&&feature_scaled_size(121,1.5)==182,"tile planner disagrees with worker DPI rounding");
    for(double scale:{1.,1.75,2.5}){
        VisualRect origin{500,400,180,45};int width=int(1800*scale),height=int(1200*scale);
        auto tiles=feature_search_tiles(origin,500,scale,width,height);
        require(tiles.size()>1,"large search was not tiled");
        int tw=int(std::round(origin.width*scale)),th=int(std::round(origin.height*scale));uint64_t candidates=0;
        for(auto tile:tiles){
            require(tile.left>=0&&tile.top>=0&&tile.right<=width&&tile.bottom<=height,"search tile outside client");
            require(uint64_t(tile.right-tile.left)*(tile.bottom-tile.top)<=524288,"search tile exceeded pixel budget");
            require(tile.right-tile.left>=tw&&tile.bottom-tile.top>=th,"search cut the template");
            candidates+=uint64_t(tile.right-tile.left-tw+1)*(tile.bottom-tile.top-th+1);
        }
        int l=std::max(0,int(std::floor((origin.x-500)*scale))),t=std::max(0,int(std::floor((origin.y-500)*scale)));
        int r=std::min(width-tw,int(std::ceil((origin.x+500)*scale))),b=std::min(height-th,int(std::ceil((origin.y+500)*scale)));
        require(candidates==uint64_t(r-l+1)*(b-t+1),"search candidate domain has gaps or overlaps");
    }
    VisualTrackingState policy;policy.featureRecovery=true;policy.searchLimit=500;
    policy.bind_session(123);policy.calibrate({100,100,180,45},168);policy.permit(10);
    for(int i=0;i<5;i++){
        double now=10.4+i*.2;auto ticket=policy.begin(now);
        require(policy.observe(ticket,i+1,now,now+.01,VisualObservation::NotFound),"search miss was not consumed");
    }
    require(!policy.locked()&&!policy.visible(),"temporary feature loss forced manual reselection");
    require(policy.allowed({590,100,180,45})&&!policy.allowed({601,100,180,45}),"feature recovery range is not 500 DIP");
    policy.featureOrigin={650.2857142857143,300,180,45};
    require(policy.allowed({150.28571428571428,130,180,45}),"fractional DPI rejected inclusive 500 DIP boundary");
    auto stale=policy.begin(11.6);policy.pause("geometry_changed");
    require(!policy.observe(stale,99,11.6,11.61,VisualObservation::Complete,{400,100,180,45}),"old resize result accepted");
    VisualTrackingState transport;transport.featureRecovery=true;transport.bind_session(555);transport.calibrate({100,100,180,45},168);
    transport.phase=VisualPhase::Following;transport.permitted=true;transport.boundary=transport.reference;
    transport.lastCheck=transport.lastAudit=10;
    transport.tick(12,true);require(transport.visible()&&transport.lastSample==0,"transport invalidated geometry or fabricated a frame");
    auto transported=transport.begin(12,true,false,true);
    require(transport.observe(transported,1,12,12.01,VisualObservation::Complete,transport.reference,true)&&transport.visible(),"fresh post-translation audit hid an unchanged target");
    // Explicit watch probe stays lightweight even well past the 1s continuous-audit horizon.
    auto watchUnchanged=transport.begin_watch(25,true);
    require(watchUnchanged.request&&!watchUnchanged.complete,"watch probe issued full locate instead of watch");
    require(transport.observe(watchUnchanged,2,25,25.01,VisualObservation::Unchanged,{},true)&&transport.visible(),"unchanged watch probe disrupted Following");
    auto watchChanged=transport.begin_watch(26,true);
    require(watchChanged.request&&!watchChanged.complete,"second watch probe failed to issue");
    require(transport.observe(watchChanged,3,26,26.01,VisualObservation::Changed,{},true)&&transport.phase==VisualPhase::Verifying&&transport.reason=="image_changed",
        "changed watch probe did not escalate to Verifying");
    require(!transport.begin_watch(26.1,true).request,"watch probe issued while Verifying");
    auto escalated=transport.begin(26.1,true,false,true);
    require(escalated.request&&escalated.complete,"escalated round did not issue full locate");
    FeatureSearch resized;
    resized.set_resize_predictions({400,200,120,40},1,1.5,1200,800);
    require(resized.predictions.empty(),"tiny resize created predictions");
    resized.set_resize_predictions({400,200,120,40},300,160,1200,800);
    require(resized.predictions.size()==5,"expected 5 distinct layout predictions");
    require(std::abs(resized.predictions[0].x-700)<1&&std::abs(resized.predictions[0].y-200)<1,"first prediction should match right shift");
    require(resized.start_predictions()&&resized.predicting&&resized.radius(500)==32,"prediction mode did not start at 32 DIP");
    while(resized.next_prediction()){}
    require(!resized.predicting,"predictions did not terminate back to radius expansion");
    resized.next_level();
    require(resized.level==1&&resized.radius(500)==128,"level 1 search radius should expand to 128 DIP");
    resized.reset();
    require(resized.predictions.empty()&&!resized.predicting&&resized.level==0,"reset did not clear predictions");
    std::ofstream(output/L"result.json")<<json{{"passed",true},{"tiled_pixel_budget",true},{"candidate_domain_complete",true},
        {"temporary_loss_not_locked",true},{"range_500_dip",true},{"stale_resize_result_rejected",true},
        {"input_probe_watch_and_escalation",true},{"resize_predictions",true}}.dump(2);
}
