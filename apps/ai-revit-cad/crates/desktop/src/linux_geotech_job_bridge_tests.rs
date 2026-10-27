//! Typed metadata counterexamples. Empty Areas below are UNIT FIXTURES, never
//! Cx focus/GUI/worker evidence. Explicit synthetic fixture setup is separate;
//! metadata operations under test never submit a job or call a numerical function.
use super::*;
#[test]fn typed_and_parsed_record_digests_join_without_losing_u64_or_array_order(){
    #[derive(Serialize)]struct Record{z:u64,a:Vec<Value>}
    let record=Record{z:9007199254740993,a:vec![json!({"z":1,"a":2}),json!(3)]};
    let parsed:Value=serde_json::from_str(r#"{"z":9007199254740993,"a":[{"z":1,"a":2},3]}"#).unwrap();
    assert_eq!(digest(&record).unwrap(),digest(&parsed).unwrap());
    let expected=r#"{"a":[{"a":2,"z":1},3],"z":9007199254740993}"#;
    assert_eq!(digest(&record).unwrap(),format!("{:x}",Sha256::digest(expected.as_bytes())));
    let mut changed=parsed.clone();changed["z"]=json!(9007199254740994u64);assert_ne!(digest(&parsed).unwrap(),digest(&changed).unwrap());
    changed=parsed.clone();changed["a"].as_array_mut().unwrap().reverse();assert_ne!(digest(&parsed).unwrap(),digest(&changed).unwrap());
}
fn fixture()->(GuiHooks,JobTicket,host::Submission){
    let fixtures=ai_revit_core::test_fixtures::ensure_circular_archive_fixtures();
    let doc=Document::open(fixtures.join("specified.native4.json")).unwrap();
    let case=doc.circular_cases().values().next().unwrap();let ground=doc.ground_sections().get(&case.definition.ground_section_id).unwrap();
    let resolved=doc.resolve_circular_editor_input(case.analysis_id).unwrap();
    let binding=crate::geotech_circular_ui::DocumentBinding{document_uuid:doc.document_uuid.to_string(),session_id:"unit-session".into(),open_generation:1,revision:doc.revision()};
    let identity=crate::geotech_circular_ui::CalculationIdentity{document:binding.clone(),ground_id:ground.ground_section_id,analysis_id:case.analysis_id,
        ground_source_operation_id:ground.source_operation_id,case_source_operation_id:case.source_operation_id,config:resolved.config,
        source_revision:"unit-source".into(),input_fingerprint:"unit-input".into(),source_fingerprint:"unit-fingerprint".into(),solver_version:"unit-version".into()};
    let ticket=JobTicket{sequence:3,search_generation:1,kind:JobKind::Analyze,identity:identity.clone(),search_spec:None};
    let editor=EditorContext{load_generation:1,resolved_identity:Some(identity),document:binding,draft_generation:1,ground_generation:1,case_generation:1,
        search_generation:1,ground_source:Some((ground.ground_section_id,ground.source_operation_id)),case_source:Some((case.analysis_id,case.source_operation_id)),
        loaded_config:Some(resolved.config),frozen_search:None};
    let envelope=host::Submission{format:"unit-only-not-submitted".into(),sequence:host::DecimalU64(7),expected_revision:host::DecimalU64(doc.revision()),
        binding:host::HostBinding{document_uuid:doc.document_uuid.to_string(),runtime_id:doc.runtime_instance_id(),session_id:"unit-session".into(),open_generation:host::DecimalU64(1),
            case_id:host::DecimalU64(case.analysis_id.get()),ground_id:host::DecimalU64(ground.ground_section_id.get()),ground_source_operation:host::DecimalU64(ground.source_operation_id.get()),
            case_source_operation:host::DecimalU64(case.source_operation_id.get()),solver_version:"unit-version".into(),solver_source_fingerprint:"unit-fingerprint".into(),mode:host::SubmissionMode::Specified},
        resolved_case_config:resolved.config,request:host::FullRequest::Specified{input:resolved.input,config:resolved.config}};
    let base=Snapshot{serial:1,redraw:1,created:Instant::now(),binding:life::Binding{profile_id:life::PROFILE.into(),run_id:"unit-run".into(),session_id:"unit-session".into(),owner:"unit-owner".into(),
        pid:1,start_ticks:1,pid_namespace:"pid:[1]".into(),elf_sha256:"a".repeat(64),window:life::WindowKey{index:0,generation:0}},window:Value::Null,model:Value::Null,canvas:Value::Null,targets:vec![]};
    let shape=Shape{elements:BTreeMap::new(),grounds:BTreeMap::new(),cases:BTreeMap::new(),archives:BTreeMap::new(),operations:BTreeMap::new()};
    let version=state::Version{uuid:doc.document_uuid.to_string(),runtime:doc.runtime_instance_id(),generation:1,revision:doc.revision(),semantic:[1;32],archive:None,report:None};
    let before=Issued{base,role:Role::Analyze,control:Control{uid:1,area:Area::Empty,rect:Rect::default(),clip:Rect::default(),enabled:true,visible:true},
        focus:Focus{uid:2,area:Area::Empty,rect:Rect::default(),clip:Rect::default(),actual:false,text:"unit".into(),generation:1},editor,next_ui:3,version:version.clone(),shape,fp:Value::Null,compute:None,report:None};
    let mut pair=state::Pair::new(1.0,2.0,3.0).unwrap();pair.sent(state::Member::Down).unwrap();pair.admit(pair.tuple(state::Member::Down)).unwrap();pair.returned(state::Member::Down).unwrap();
    let command=Command{before,pair,clicked:true,intent_started:true,intent_returned:false,intent_ok:false,job_ticket:Some(ticket.clone()),report_ticket:None,
        envelope:None,submit_returned:false,accepted:None,report_key:None,report_sequence:None,report_start_returned:false,archive:state::ArchiveTransition::new(version),
        commit_attempted:false,commit:None,post_editor:None,terminal:None,complete:false,error:None,polls:0,cleanup_returned:false,focus_after_down:None};
    let mut hooks=GuiHooks::default();hooks.jobs.pending=Some("unit-command".into());hooks.jobs.operations.insert("unit-command".into(),command);(hooks,ticket,envelope)
}
#[test]fn no_pending_or_mismatched_ticket_cannot_consume_submit_attempt(){
    let(mut hooks,mut ticket,envelope)=fixture();ticket.sequence+=1;
    assert!(job_native_start(&mut hooks,&ticket,None).is_err());assert!(job_submit_before(&mut hooks,&ticket,&envelope).is_err());
    assert_eq!(hooks.jobs.submit_attempts,0);assert_eq!(super::super::persistence::actual_submit_attempts(&hooks.persistence),0);
    hooks.jobs.pending=None;assert!(job_native_start(&mut hooks,&ticket,None).is_err());
}
#[test]fn actual_call_attempt_and_lost_return_are_not_accepted_or_replayable(){
    let(mut hooks,ticket,envelope)=fixture();job_native_start(&mut hooks,&ticket,None).unwrap();job_submit_before(&mut hooks,&ticket,&envelope).unwrap();
    assert_eq!(hooks.jobs.submit_attempts,1);assert_eq!(super::super::persistence::actual_submit_attempts(&hooks.persistence),1);assert_eq!(hooks.jobs.submit_accepted,0);
    assert!(job_submit_before(&mut hooks,&ticket,&envelope).is_err());job_submit_returned(&mut hooks,&ticket,&envelope,None).unwrap();
    assert!(job_submit_returned(&mut hooks,&ticket,&envelope,None).is_err());assert_eq!(hooks.jobs.submit_attempts,1);assert_eq!(hooks.jobs.submit_accepted,0);
    let c=&hooks.jobs.operations["unit-command"];assert_eq!(c.pair.certainty(),state::Certainty::OutcomeUnknown);assert!(c.pair.release_required());assert!(c.accepted.is_none());
}
#[test]fn mismatched_success_sequence_never_becomes_actual_acceptance(){
    let(mut hooks,ticket,envelope)=fixture();job_submit_before(&mut hooks,&ticket,&envelope).unwrap();
    let status=host::HostStatus{sequence:host::DecimalU64(8),phase:native_jobs::JobPhase::Running,request_digest:"unit-request".into(),captured_project_revision:envelope.expected_revision,
        result_available:false,worker_owned:true,progress:None,progress_updates:host::DecimalU64(0),failure:None,worker_arithmetic:None,core_status:None};
    assert!(job_submit_returned(&mut hooks,&ticket,&envelope,Some(&status)).is_err());assert_eq!(hooks.jobs.submit_attempts,1);assert_eq!(hooks.jobs.submit_accepted,0);
    let c=&hooks.jobs.operations["unit-command"];assert!(c.submit_returned);assert!(c.accepted.is_none());assert_eq!(c.pair.certainty(),state::Certainty::OutcomeUnknown);
}
