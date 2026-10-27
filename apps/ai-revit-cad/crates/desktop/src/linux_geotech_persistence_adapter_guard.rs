//! Pure consumer admission predicates. Callers supply actual App observations.
use crate::linux_geotech_persistence_contract::Target;
pub fn protocol_version(method:&str)->u64 {
    match method {"diagnostic.gui.input"|"diagnostic.gui.capture_document"=>2,_=>1}
}
pub fn handler_permitted(expected:Target,observed:Target,expected_uid:u64,actual_uid:Option<u64>,
    down_admitted:bool,clicked:bool,already_started:bool,pending_exact:bool)->bool {
    matches!(expected,Target::SaveDocument|Target::OpenDocument|Target::NewDocument)
        &&expected==observed&&expected_uid!=0&&actual_uid==Some(expected_uid)
        &&down_admitted&&clicked&&!already_started&&pending_exact
}
pub fn archive_inspection_window(target:Target,handler_started:bool,hash_verified:bool,
    calculation_quiet:bool,complete:bool)->bool {
    target==Target::OpenDocument&&handler_started&&hash_verified&&calculation_quiet&&!complete
}
#[cfg(test)]mod tests {
    use super::*;
    #[test]fn api_v2_cannot_widen_public_document_methods_or_legacy_capture(){
        assert_eq!(protocol_version("diagnostic.gui.input"),2);
        assert_eq!(protocol_version("diagnostic.gui.capture_document"),2);
        for m in ["document.open","document.save","diagnostic.gui.click","diagnostic.gui.capture","diagnostic.gui.input.extra"] {
            assert_eq!(protocol_version(m),1);
        }
    }
    #[test]fn ordinary_handler_permit_refuses_wrong_uid_target_replay_and_unowned_pending(){
        assert!(handler_permitted(Target::OpenDocument,Target::OpenDocument,81,Some(81),true,true,false,true));
        assert!(!handler_permitted(Target::OpenDocument,Target::SaveDocument,81,Some(81),true,true,false,true));
        assert!(!handler_permitted(Target::OpenDocument,Target::OpenDocument,81,Some(82),true,true,false,true));
        assert!(!handler_permitted(Target::OpenDocument,Target::OpenDocument,81,Some(81),false,true,false,true));
        assert!(!handler_permitted(Target::OpenDocument,Target::OpenDocument,81,Some(81),true,true,true,true));
        assert!(!handler_permitted(Target::OpenDocument,Target::OpenDocument,81,Some(81),true,true,false,false));
        assert!(!handler_permitted(Target::PathInput,Target::PathInput,81,Some(81),true,true,false,true));
    }
    #[test]fn archive_inspection_exception_cannot_admit_calculation_or_unverified_open(){
        assert!(archive_inspection_window(Target::OpenDocument,true,true,true,false));
        assert!(!archive_inspection_window(Target::OpenDocument,true,true,false,false));
        assert!(!archive_inspection_window(Target::OpenDocument,true,false,true,false));
        assert!(!archive_inspection_window(Target::SaveDocument,true,true,true,false));
        assert!(!archive_inspection_window(Target::OpenDocument,true,true,true,true));
    }
}
